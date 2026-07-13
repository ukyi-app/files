use super::atomic;
use super::pins::{Hooks, PassGuard, Settled};
use super::Store;
use crate::layout::{classify_objects_entry, grave_sha, Layout, ObjectsEntry};
use crate::meta::ObjectMeta;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// reconciliation 1회 결과(관측성·테스트용).
///
/// # ⚠ 이 구조체에 **필드를 추가하지 마라**
///
/// `tests/layout_tree.rs:71,137,198`이 **구조체 전수 `assert_eq!`**로 stats를 핀한다 → 필드를 하나라도
/// 늘리면 그 3개가 깨진다 = **두 번째 관측 행동 플립**(gated-bugfix 하드룰 10 위반).
/// **무덤 복구 수(`recovered`) · 복원 수(`restored`) · 연기 수(`deferred`)는 전부 `tracing` 전용**이며
/// 여기에 올라오지 않는다. 연기 카운터를 stats/metrics로 내보내고 싶으면 **stats 계약을 여는 별도
/// 파이프라인**(**F-29**)이다.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    /// 커밋 포인터가 참조하는 **서로 다른 sha 수**(이 패스의 스냅샷).
    pub referenced: usize,
    /// **회수(reap)된 blob 수.** 무덤으로 옮겼다가 **복원**된 blob(`Settled::Restored`)과 **연기**된
    /// blob(`Settled::Deferred`)은 **세지 않는다** — **회수하지 않았기 때문이다.**
    /// (`Restored`/`Deferred`는 tombstone도 유지한다 — D-2: 다음 패스가 새 스냅샷으로 재판정한다.)
    pub gc_deleted: usize,
    /// tombstone 대기 중인(= 무참조로 관측됐으나 아직 grace 내인) blob 수.
    pub gc_pending: usize,
    /// grace를 넘긴 temp 잔재 삭제 수(`.tmp-` 접두 — 무덤은 여기 **절대** 들어오지 않는다).
    pub temps_deleted: usize,
    /// 비트로트 격리 수(내용 sha ≠ 이름). ⚠ **이 분기는 핀·무덤을 거치지 않는다**(D-4 → **F-25**).
    pub quarantined: usize,
}

/// 무취소 커밋 **꼬리**의 여유분. 이 꼬리는 `commit_pointer`의 blocking 클로저가 rename 전후로
/// 수행하는 **고정 크기 작업**이다: `mkdir_p`, `create`, `write_all`(**메타 JSON 수백 바이트**),
/// `sync_all(file)`, `rename`, `sync_all(parent)`. 업로드 **크기에 비례하지 않는다**
/// → 여유분은 **상수**가 맞다(비율 아님). 건강한 디스크에서 한 자릿수 ms · blocking 풀이 대형
/// 스크럽으로 포화돼도 1초 미만. **60초 = 그 위로 두 자릿수 배의 헤드룸**이다.
pub const GC_SETTLE_MARGIN: Duration = Duration::from_secs(60);

/// **명시적 상계.** `upload_timeout`에서 **파생**하되 — ⚠ **`upload_timeout`은 상계가 아니다**
/// (시작된 `spawn_blocking` 클로저는 abort 불가하므로 호출자 타임아웃이 그것을 죽이지 못한다).
pub fn settle_timeout_from(upload_timeout: Duration) -> Duration {
    upload_timeout + GC_SETTLE_MARGIN
}

/// 미참조 blob GC + 활성 temp 보존 + bit-rot 격리. `SystemTime::now()`로 위임.
///
/// ⚠ `store`는 **경로가 아니라 `&Store`**다(D-1) — 핀 등록부가 in-process이므로 GC는 put과
/// **같은 `Store`**를 봐야 한다. `settle_timeout`은 **명시 인자**다: 기본값을 숨기지 않는다.
/// 그것이 대기의 **유일한 상계**이므로 호출자가 **알고 정해야** 한다.
/// prod = `settle_timeout_from(cfg.upload_timeout)`.
pub async fn run_once(
    store: &Store,
    gc_grace: Duration,
    settle_timeout: Duration,
) -> std::io::Result<ReconcileStats> {
    run_once_at(store, SystemTime::now(), gc_grace, settle_timeout).await
}

/// 전 버킷 커밋 포인터를 워크해 `*.meta.json`이 가리키는 sha 집합 수집.
/// 순회·이름 규칙(루트 직속 파일 배제·`.objects` 스킵·temp 제외·재귀)은 워커 소유(R-4).
/// (발견 P2-1: 비재귀 글롭은 중첩 키 blob을 미참조로 오인 — 워커가 재귀로 커버)
/// 여기 남는 정책: 워커가 낸 포인터의 read/파싱 실패는 조용히 skip(B7).
pub(super) async fn collect_referenced(
    layout: &Layout,
    hooks: &Hooks,
) -> std::io::Result<HashSet<String>> {
    let mut refs = HashSet::new();
    let mut walk = layout.pointers_all();
    // 워커의 io::Error는 무가공 전파(B7) — reconcile은 std::io::Result를 반환한다.
    while let Some(entry) = walk.next().await? {
        if let Ok(raw) = tokio::fs::read(&entry.meta_path).await {
            if let Ok(meta) = serde_json::from_slice::<ObjectMeta>(&raw) {
                hooks.during_collect(&meta.sha256).await; // 결정적 배리어
                refs.insert(meta.sha256);
            }
        }
    }
    Ok(refs)
}

/// 잔존 무덤 **보수적** 복구 — `PassGuard::begin`이 collect **이전에** 호출한다.
/// 무덤은 `settle()`이 `?`로 탈출했거나 프로세스가 죽었을 때만 남는다(fail-CLOSED by construction).
///
/// * blob 부재 → `rename(grave → blob)` (복구)
/// * blob 존재 ∧ 내용 sha == sha → `remove_file(grave)` (정본이 검증 통과 → 무덤 폐기)
/// * blob 존재 ∧ 내용 sha != sha → `rename(grave → blob)` (정본이 썩었다 → **무덤을 채택**)
///
/// 어느 경우든 이번 패스의 `Blob` 분기가 내용을 재검증한다. 반환 = 정본으로 되돌린 무덤 수.
/// clean 트리에서는 **no-op**이다(무덤이 없으므로).
pub(super) async fn recover_graves(layout: &Layout) -> std::io::Result<usize> {
    let objects = layout.objects_dir();
    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&objects).await?;
    while let Some(e) = rd.next_entry().await? {
        entries.push(e);
    }

    let mut recovered = 0usize;
    for e in entries {
        let name = e.file_name();
        let name = name.to_string_lossy().to_string();
        let Some(sha) = grave_sha(&name).map(str::to_owned) else {
            continue; // 무덤 이름이 아니다
        };
        // 무덤은 rename으로만 태어난다 → 디렉터리일 수 없다. 디렉터리면 **건드리지 않는다**
        // (무검증 파괴 경로 제거).
        if e.file_type().await?.is_dir() {
            continue;
        }
        let grave = e.path();
        let blob = layout.blob_path(&sha);
        let blob_intact = matches!(
            tokio::fs::read(&blob).await,
            Ok(b) if hex::encode(Sha256::digest(&b)) == sha
        );
        if blob_intact {
            tokio::fs::remove_file(&grave).await?;
            atomic::fsync_dir(&objects).await?;
        } else {
            atomic::rename_durable(&grave, &blob, &objects).await?;
            recovered += 1;
            // 관측성: 잔존 무덤을 **정본으로 되돌렸다**. 무덤이 남아 있었다는 것은 지난 패스가
            // `?`로 탈출했거나 프로세스가 죽었다는 뜻이다 — fail-CLOSED가 **작동한** 증거다.
            tracing::info!(sha = %sha, "grave recovered");
        }
    }
    Ok(recovered)
}

/// `now` 주입형 reconciliation(테스트 결정성).
async fn run_once_at(
    store: &Store,
    now: SystemTime,
    gc_grace: Duration,
    settle_timeout: Duration,
) -> std::io::Result<ReconcileStats> {
    let layout = store.layout();
    let objects = layout.objects_dir();
    let mut stats = ReconcileStats::default();
    if !tokio::fs::try_exists(&objects).await? {
        return Ok(stats);
    }

    // 패스 등록 → 무덤 복구 → 참조 스냅샷. 이 셋의 순서는 PassGuard가 소유한다(P5).
    // ⚠ `recover_graves`가 `collect_referenced`**보다 먼저** 돈다 — 뒤집으면 크래시 창에 커밋된
    //    포인터가 refs에 안 잡힌다(근거: `PassGuard::begin`의 순서 제약 ③).
    let pass = PassGuard::begin(store, settle_timeout).await?;
    // 관측성 — **`ReconcileStats`에는 필드를 추가하지 않는다**(tracing 전용). clean 트리에서는 0이므로
    // 침묵한다. 0이 아니라는 것은 **지난 패스가 무덤을 흘렸다**(크래시/`?` 탈출)는 뜻이며, 이 패스가
    // 그것을 정본으로 되돌렸다는 뜻이다 — 개별 sha는 `recover_graves`가 이미 로그했다.
    if pass.recovered() > 0 {
        tracing::info!(
            recovered = pass.recovered(),
            "graves recovered from a previous pass"
        );
    }
    let refs = pass.referenced();
    stats.referenced = refs.len();

    let pending_path = layout.gc_pending_path();
    let mut pending: HashMap<String, u64> = match tokio::fs::read(&pending_path).await {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or_default(),
        Err(_) => HashMap::new(),
    };
    let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let grace_secs = gc_grace.as_secs();
    let corrupt_dir = layout.corrupt_dir();

    // .objects 직속 항목 스냅샷(순회 중 변경 회피)
    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&objects).await?;
    while let Some(e) = rd.next_entry().await? {
        entries.push(e);
    }

    for e in entries {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy().to_string();
        // 이름-전용 분류(I/O 없음). Temp가 Blob보다 우선하고 대문자 hex도 Blob이다
        // (정규화 없음 — 내용 검증에서 격리되는 현행 B6 보존).
        let class = classify_objects_entry(&name);
        // O1: 예약 이름(.gc-pending.json/.corrupt)은 file_type 조회 **전에** continue.
        // stat을 걸지 않는 현행 syscall 순서를 그대로 유지한다.
        if matches!(class, ObjectsEntry::Reserved) {
            continue;
        }
        // **결정적 배리어** — 이 항목의 **첫 FS 접촉 직전**(= `file_type()` 이전 · 예약 이름 `continue`
        // **이후** ⇒ O1의 "예약 이름에는 stat을 걸지 않는다"가 보존된다). prod = `None` ⇒ 즉시 반환
        // (syscall 0 · 상태 변경 0) ⇒ **관측 행동 변화 0**.
        // 이 창이 없으면 **Temp 분기**(아래 `e.metadata().await?`)의 소멸 레이스를 결정적으로 재현할
        // 방법이 없다 — `pre_grave`는 **Blob 분기 전용**이라 temp에는 영영 발화하지 않는다.
        // 반환값은 `()`다 → 이 훅은 아래 분기 판정에 **개입할 수 없다**(P4 봉인 유지).
        pass.pins().hooks().pre_entry(&name).await;
        // O2: 디렉터리 스킵은 temp/blob 처리보다 앞.
        let ft = e.file_type().await?;
        if ft.is_dir() {
            continue;
        }
        match class {
            // 3) temp 잔재: mtime이 grace보다 오래된 것만 삭제(활성 스트리밍 보존)
            ObjectsEntry::Temp => {
                let mtime = e.metadata().await?.modified().unwrap_or(now);
                let age = now.duration_since(mtime).unwrap_or_default();
                if age.as_secs() > grace_secs {
                    tokio::fs::remove_file(&p).await?;
                    stats.temps_deleted += 1;
                }
            }
            ObjectsEntry::Blob => {
                // 4) 무결성: 내용 sha == 파일명 검증, 불일치 → 격리
                let content = tokio::fs::read(&p).await?;
                if hex::encode(Sha256::digest(&content)) != name {
                    atomic::mkdir_p_durable(&corrupt_dir).await?;
                    tokio::fs::rename(&p, corrupt_dir.join(&name)).await?;
                    atomic::fsync_dir(&objects).await?;
                    pending.remove(&name);
                    stats.quarantined += 1;
                    tracing::warn!(sha = %name, "quarantined corrupt blob (bit rot)");
                    continue;
                }
                // 2) 2단계 tombstone GC: 미참조 지속시간 기준
                if refs.contains(&name) {
                    pending.remove(&name); // 다시 참조됨
                } else {
                    match pending.get(&name) {
                        Some(&first) if now_secs.saturating_sub(first) > grace_secs => {
                            // 결정적 배리어(= 모델링된 **사전확인 지점**). `reconcile.rs`가
                            // `BlobPins`에서 얻을 수 있는 것은 **훅뿐**이다(P4) — `live`/`landed`를
                            // 읽을 방법이 아예 없다 → 여기서 보호 여부를 미리 판정하는 뮤턴트는
                            // 이 모듈에서 **표현 불가**다.
                            pass.pins().hooks().pre_grave(&name).await;
                            // `settle()`은 `Graved`의 메서드이고 `Graved`는 `grave()`의 rename이
                            // 성공해야만 태어난다 → 두 호출을 뒤바꾸는 뮤턴트는 **컴파일되지 않는다**.
                            match pass.grave(&name).await?.settle().await? {
                                Settled::Reaped => {
                                    pending.remove(&name);
                                    stats.gc_deleted += 1;
                                }
                                // D-2: tombstone **유지** · 무카운트. 다음 패스가 새 스냅샷으로 재판정한다.
                                Settled::Restored => {
                                    tracing::info!(sha = %name, "GC restored: landed commit");
                                }
                                // **degraded 경로**(P7 fail-CLOSED). 무덤은 이미 정본으로 복원됐다.
                                // 에러 로그는 `settle()`이 이미 냈다(중복 로깅 금지).
                                // ⚠ `?`로 패스를 중단하지 않는다 — 멈춘 핀 **하나**가 다른 blob들의
                                //    GC를 막으면 안 된다. 루프는 **계속 돈다**.
                                Settled::Deferred => {}
                            }
                        }
                        Some(_) => {} // 아직 grace 내 — 보존
                        None => {
                            pending.insert(name.clone(), now_secs); // 최초 관측
                        }
                    }
                }
            }
            // 도달 불가(recover_graves가 패스 시작에 비웠다). **아무것도 하지 않는다** —
            // 무덤은 유일한 사본일 수 있으므로 절대 삭제 금지. 다음 패스가 복구한다.
            ObjectsEntry::Grave => {}
            // Reserved는 위(O1)에서 이미 continue. 그 외 이름은 조용히 무시(현행 !is_sha).
            ObjectsEntry::Reserved | ObjectsEntry::Other => {}
        }
    }

    // 존재하지 않는 blob의 pending 엔트리 정리
    let mut cleaned = HashMap::new();
    for (sha, t) in pending.into_iter() {
        if tokio::fs::try_exists(layout.blob_path(&sha)).await? {
            cleaned.insert(sha, t);
        }
    }
    stats.gc_pending = cleaned.len();
    atomic::write_atomic(&pending_path, &serde_json::to_vec(&cleaned).unwrap()).await?;

    Ok(stats)
}

/// **테스트 전용 다리(S-3).** B-2의 배리어 증인은 **두 기능을 같은 테스트 안에서** 요구한다:
/// ① `Hooks` 구성 — 8개 필드가 **`pins.rs` private**이라 그 모듈(과 그 `mod tests`) 안에서만
/// 리터럴로 지을 수 있다 · ② **주입형 시각**의 reconciler — `run_once_at`은 **이 모듈 private**이다.
/// 이 둘이 형제 private 모듈로 갈라져 있으면 `pins.rs`의 증인은 훅을 짓고도 시계를 주입할 수 없고,
/// `reconcile.rs`의 증인은 그 반대다 → B-2의 안무(§6: `run_once_at` + `Hooks{pre_grave, post_grave, …}`)를
/// **구성할 방법이 없다**. 이 다리가 그 벽을 **`store` 모듈 안에서만** 뚫는다.
///
/// **프로덕션 표면은 한 글자도 넓어지지 않는다**:
/// * `run_once_at`은 여전히 **이 모듈 private**(`pub` 아님) — 밖에서 부를 수 없다.
/// * 보호 상태(`landed`/`live`)와 `Hooks`의 **8개 필드는 `pins.rs` private 그대로**다.
/// * 이 래퍼는 `#[cfg(test)]` → **릴리스 빌드에 존재하지 않는다.**
/// * 위임 외에 **아무 일도 하지 않는다** — 주입형-시각 안무를 약화시키지 않는다.
#[cfg(test)]
pub(super) async fn run_once_at_for_test(
    store: &Store,
    now: SystemTime,
    gc_grace: Duration,
    settle_timeout: Duration,
) -> std::io::Result<ReconcileStats> {
    run_once_at(store, now, gc_grace, settle_timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{atomic, Store};
    use sha2::{Digest, Sha256};
    use std::time::{Duration, SystemTime};

    /// 넉넉한 예산 — B-1에서는 무덤이 만들어지지 않으므로 settle이 발화하지 않는다.
    const SETTLE: Duration = Duration::from_secs(30);

    fn hex_sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    async fn write_obj_file(root: &std::path::Path, name: &str, content: &[u8]) {
        atomic::write_atomic(&root.join(".objects").join(name), content)
            .await
            .unwrap();
    }

    /// `settle_timeout`은 `upload_timeout`에서 **파생**된다(새 env 노브 없음) — 기본값 600s → 660s.
    /// 파생이 **단조**여야 운영자가 `FILES_UPLOAD_TIMEOUT`을 올렸을 때 **정상적으로 느린 put이
    /// 타임아웃되지 않는다**(정상 경로 연기 = 0 유지).
    #[test]
    fn settle_timeout_derives_from_upload_timeout_and_is_monotonic() {
        assert_eq!(
            settle_timeout_from(Duration::from_secs(600)),
            Duration::from_secs(660)
        );
        assert_eq!(
            settle_timeout_from(Duration::from_secs(600)),
            Duration::from_secs(600) + GC_SETTLE_MARGIN
        );
        // 단조: upload_timeout을 올리면 settle_timeout도 오른다
        let mut prev = settle_timeout_from(Duration::ZERO);
        for s in [1u64, 10, 600, 3600] {
            let cur = settle_timeout_from(Duration::from_secs(s));
            assert!(cur > prev, "settle_timeout 파생은 단조여야 함");
            prev = cur;
        }
        // 그리고 항상 upload_timeout보다 크다(무취소 커밋 꼬리의 여유분)
        assert!(settle_timeout_from(Duration::from_secs(600)) > Duration::from_secs(600));
    }

    #[tokio::test]
    async fn referenced_nested_blob_survives() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let s = Store::new(root.to_path_buf());
        let m = s
            .put("b", "a/b.zip", "x", "u", b"nested".to_vec())
            .await
            .unwrap();
        let stats = run_once(&s, Duration::from_secs(3600), SETTLE).await.unwrap();
        assert!(
            tokio::fs::try_exists(s.blob_path(&m.sha256)).await.unwrap(),
            "참조된 중첩 키 blob은 생존해야 함"
        );
        assert_eq!(stats.gc_deleted, 0);
        assert!(stats.referenced >= 1);
    }

    #[tokio::test]
    async fn unreferenced_old_blob_is_gced() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let s = Store::new(root.to_path_buf());
        tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
        let content = b"orphan".to_vec();
        let sha = hex_sha(&content);
        write_obj_file(root, &sha, &content).await;
        let grace = Duration::from_secs(100);
        let t0 = SystemTime::now();
        run_once_at(&s, t0, grace, SETTLE).await.unwrap(); // 최초 관측 → pending
        assert!(tokio::fs::try_exists(root.join(".objects").join(&sha)).await.unwrap());
        let stats = run_once_at(&s, t0 + Duration::from_secs(101), grace, SETTLE).await.unwrap();
        assert!(!tokio::fs::try_exists(root.join(".objects").join(&sha)).await.unwrap());
        assert_eq!(stats.gc_deleted, 1);
    }

    #[tokio::test]
    async fn unreferenced_recent_blob_preserved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let s = Store::new(root.to_path_buf());
        tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
        let content = b"fresh".to_vec();
        let sha = hex_sha(&content);
        write_obj_file(root, &sha, &content).await;
        let grace = Duration::from_secs(3600);
        let t0 = SystemTime::now();
        run_once_at(&s, t0, grace, SETTLE).await.unwrap();
        let stats = run_once_at(&s, t0 + Duration::from_secs(1), grace, SETTLE).await.unwrap();
        assert!(
            tokio::fs::try_exists(root.join(".objects").join(&sha)).await.unwrap(),
            "grace 내 최근 미참조 blob은 보존되어야 함"
        );
        assert_eq!(stats.gc_deleted, 0);
    }

    #[tokio::test]
    async fn corrupt_blob_quarantined() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let s = Store::new(root.to_path_buf());
        tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
        let bad_name = "0".repeat(64); // 이름 ≠ sha(content)
        write_obj_file(root, &bad_name, b"not matching content").await;
        let stats = run_once(&s, Duration::from_secs(3600), SETTLE).await.unwrap();
        assert_eq!(stats.quarantined, 1);
        assert!(!tokio::fs::try_exists(root.join(".objects").join(&bad_name)).await.unwrap());
        assert!(tokio::fs::try_exists(root.join(".objects").join(".corrupt").join(&bad_name)).await.unwrap());
    }

    #[tokio::test]
    async fn old_temp_deleted_recent_preserved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let s = Store::new(root.to_path_buf());
        let objects = root.join(".objects");
        tokio::fs::create_dir_all(&objects).await.unwrap();
        write_obj_file(root, ".tmp-stream", b"in flight").await;
        let grace = Duration::from_secs(100);
        run_once_at(&s, SystemTime::now(), grace, SETTLE).await.unwrap();
        assert!(
            tokio::fs::try_exists(objects.join(".tmp-stream")).await.unwrap(),
            "최근 temp는 보존"
        );
        let stats = run_once_at(&s, SystemTime::now() + Duration::from_secs(300), grace, SETTLE)
            .await
            .unwrap();
        assert!(
            !tokio::fs::try_exists(objects.join(".tmp-stream")).await.unwrap(),
            "오래된 temp는 삭제"
        );
        assert_eq!(stats.temps_deleted, 1);
    }

    // ── `recover_graves`의 두 가드 (T-Q2 · T-Q3) ──────────────────────────────────────────
    // 훅이 **필요 없는** 증인이므로 이 모듈에 산다(다리 불필요 — `run_once`를 직접 부른다).
    // **park 0 · spawn 0 · 동시 put 0** — "확인 안 함"이 아니라 "확인했고 없음"이다.

    /// 커밋 포인터를 손으로 심는다(put을 거치지 않는다 — put은 blob이 없으면 **바이트를 재기록**해
    /// 복구를 가려 버린다). 이 테스트들의 관심사는 `recover_graves` **그 자체**다.
    async fn write_pointer(root: &std::path::Path, bucket: &str, key: &str, sha: &str, size: u64) {
        let meta = crate::meta::ObjectMeta {
            content_type: "text/plain".into(),
            size,
            sha256: sha.to_owned(),
            created_at: "2026-01-01T00:00:00Z".into(),
            uploaded_by: "u".into(),
        };
        atomic::write_atomic(
            &root.join(bucket).join(format!("{key}.meta.json")),
            &serde_json::to_vec(&meta).unwrap(),
        )
        .await
        .unwrap();
    }

    /// **T-Q2 — `recover_graves`의 내용 검증.** 정본이 **썩었고**(내용 sha ≠ 이름) 무덤에 **정상
    /// 사본**이 있으면 **무덤이 정본을 덮어쓴다**.
    ///
    /// **뮤턴트**(`blob 존재 → remove_file(grave)` **무검증**) → 좋은 사본이 소멸하고 썩은 정본만
    /// 남는다 → 블롭 루프가 그것을 **격리**한다(`quarantined == 1`) → `get_bytes` **404** → **RED**.
    #[tokio::test]
    async fn recover_graves_adopts_the_grave_when_the_canonical_blob_is_rotten() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::new(root.clone());
        let good = b"tq2-good-copy".to_vec();
        let sha = hex_sha(&good);

        // 정본은 **썩었다**(이름 ≠ 내용 sha) · 무덤에는 **정상 사본** · 포인터는 그 sha를 참조한다
        write_obj_file(&root, &sha, b"rotten bytes").await;
        atomic::write_atomic(&s.layout().grave_path(&sha), &good)
            .await
            .unwrap();
        write_pointer(&root, "b", "k", &sha, good.len() as u64).await;

        let stats = run_once(&s, Duration::from_secs(3600), SETTLE).await.unwrap();
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 1,
                gc_deleted: 0,
                gc_pending: 0,
                temps_deleted: 0,
                quarantined: 0, // 무덤이 정본을 덮어썼다 → **격리할 것이 없다**
            }
        );
        let (_, got) = s.get_bytes("b", "k").await.expect("좋은 사본이 살아남아야 한다");
        assert_eq!(got, good);
        assert!(!tokio::fs::try_exists(s.layout().grave_path(&sha)).await.unwrap());
    }

    /// **T-Q3 — `is_dir` 가드.** 무덤은 rename으로만 태어나므로 **디렉터리일 수 없다** —
    /// `.gc-grave-<64hex>` 이름의 **디렉터리**를 심으면 `recover_graves`는 **건드리지 않는다**
    /// (무검증 파괴 경로 제거). 정본이 디렉터리가 되지 않으므로 이후 put이 **500으로 영구화되지 않는다**.
    #[tokio::test]
    async fn recover_graves_skips_a_directory_that_is_named_like_a_grave() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::new(root.clone());
        let content = b"tq3-payload".to_vec();
        let sha = hex_sha(&content);
        tokio::fs::create_dir_all(s.layout().grave_path(&sha)).await.unwrap();

        let stats = run_once(&s, Duration::from_secs(3600), SETTLE).await.unwrap();
        assert_eq!(
            stats,
            ReconcileStats::default(),
            "무덤 **모양의 디렉터리**는 아무 것도 바꾸지 않는다"
        );
        assert!(
            !tokio::fs::try_exists(s.blob_path(&sha)).await.unwrap(),
            "정본이 **디렉터리가 되지 않았다**"
        );
        assert!(
            tokio::fs::try_exists(s.layout().grave_path(&sha)).await.unwrap(),
            "무덤 모양 디렉터리는 그대로 남는다(건드리지 않는다)"
        );

        // 이후 put이 정상 동작한다(영구 500 없음)
        s.put("b", "k", "text/plain", "u", content.clone())
            .await
            .expect("put은 정상 동작한다");
        let (_, got) = s.get_bytes("b", "k").await.unwrap();
        assert_eq!(got, content);
    }
}
