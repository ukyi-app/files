use super::atomic;
use crate::layout::{classify_objects_entry, Layout, ObjectsEntry};
use crate::meta::ObjectMeta;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// reconciliation 1회 결과(관측성·테스트용).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub referenced: usize,
    pub gc_deleted: usize,
    pub gc_pending: usize,
    pub temps_deleted: usize,
    pub quarantined: usize,
}

/// 미참조 blob GC + 활성 temp 보존 + bit-rot 격리. `SystemTime::now()`로 위임.
pub async fn run_once(root: &Path, gc_grace: Duration) -> std::io::Result<ReconcileStats> {
    run_once_at(root, SystemTime::now(), gc_grace).await
}

/// 전 버킷 커밋 포인터를 워크해 `*.meta.json`이 가리키는 sha 집합 수집.
/// 순회·이름 규칙(루트 직속 파일 배제·`.objects` 스킵·temp 제외·재귀)은 워커 소유(R-4).
/// (발견 P2-1: 비재귀 글롭은 중첩 키 blob을 미참조로 오인 — 워커가 재귀로 커버)
/// 여기 남는 정책: 워커가 낸 포인터의 read/파싱 실패는 조용히 skip(B7).
async fn collect_referenced(layout: &Layout) -> std::io::Result<HashSet<String>> {
    let mut refs = HashSet::new();
    let mut walk = layout.pointers_all();
    // 워커의 io::Error는 무가공 전파(B7) — reconcile은 std::io::Result를 반환한다.
    while let Some(entry) = walk.next().await? {
        if let Ok(raw) = tokio::fs::read(&entry.meta_path).await {
            if let Ok(meta) = serde_json::from_slice::<ObjectMeta>(&raw) {
                refs.insert(meta.sha256);
            }
        }
    }
    Ok(refs)
}

/// `now` 주입형 reconciliation(테스트 결정성).
async fn run_once_at(
    root: &Path,
    now: SystemTime,
    gc_grace: Duration,
) -> std::io::Result<ReconcileStats> {
    let layout = Layout::new(root.to_path_buf());
    let objects = layout.objects_dir();
    let mut stats = ReconcileStats::default();
    if !tokio::fs::try_exists(&objects).await? {
        return Ok(stats);
    }

    let refs = collect_referenced(&layout).await?;
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
                            tokio::fs::remove_file(&p).await?;
                            atomic::fsync_dir(&objects).await?;
                            pending.remove(&name);
                            stats.gc_deleted += 1;
                        }
                        Some(_) => {} // 아직 grace 내 — 보존
                        None => {
                            pending.insert(name.clone(), now_secs); // 최초 관측
                        }
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{atomic, Store};
    use sha2::{Digest, Sha256};
    use std::time::{Duration, SystemTime};

    fn hex_sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    async fn write_obj_file(root: &std::path::Path, name: &str, content: &[u8]) {
        atomic::write_atomic(&root.join(".objects").join(name), content)
            .await
            .unwrap();
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
        let stats = run_once(root, Duration::from_secs(3600)).await.unwrap();
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
        tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
        let content = b"orphan".to_vec();
        let sha = hex_sha(&content);
        write_obj_file(root, &sha, &content).await;
        let grace = Duration::from_secs(100);
        let t0 = SystemTime::now();
        run_once_at(root, t0, grace).await.unwrap(); // 최초 관측 → pending
        assert!(tokio::fs::try_exists(root.join(".objects").join(&sha)).await.unwrap());
        let stats = run_once_at(root, t0 + Duration::from_secs(101), grace).await.unwrap();
        assert!(!tokio::fs::try_exists(root.join(".objects").join(&sha)).await.unwrap());
        assert_eq!(stats.gc_deleted, 1);
    }

    #[tokio::test]
    async fn unreferenced_recent_blob_preserved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
        let content = b"fresh".to_vec();
        let sha = hex_sha(&content);
        write_obj_file(root, &sha, &content).await;
        let grace = Duration::from_secs(3600);
        let t0 = SystemTime::now();
        run_once_at(root, t0, grace).await.unwrap();
        let stats = run_once_at(root, t0 + Duration::from_secs(1), grace).await.unwrap();
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
        tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
        let bad_name = "0".repeat(64); // 이름 ≠ sha(content)
        write_obj_file(root, &bad_name, b"not matching content").await;
        let stats = run_once(root, Duration::from_secs(3600)).await.unwrap();
        assert_eq!(stats.quarantined, 1);
        assert!(!tokio::fs::try_exists(root.join(".objects").join(&bad_name)).await.unwrap());
        assert!(tokio::fs::try_exists(root.join(".objects").join(".corrupt").join(&bad_name)).await.unwrap());
    }

    #[tokio::test]
    async fn old_temp_deleted_recent_preserved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let objects = root.join(".objects");
        tokio::fs::create_dir_all(&objects).await.unwrap();
        write_obj_file(root, ".tmp-stream", b"in flight").await;
        let grace = Duration::from_secs(100);
        run_once_at(root, SystemTime::now(), grace).await.unwrap();
        assert!(
            tokio::fs::try_exists(objects.join(".tmp-stream")).await.unwrap(),
            "최근 temp는 보존"
        );
        let stats = run_once_at(root, SystemTime::now() + Duration::from_secs(300), grace)
            .await
            .unwrap();
        assert!(
            !tokio::fs::try_exists(objects.join(".tmp-stream")).await.unwrap(),
            "오래된 temp는 삭제"
        );
        assert_eq!(stats.temps_deleted, 1);
    }
}
