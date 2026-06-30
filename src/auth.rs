use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// 서비스별 API 키. keys.json(SealedSecret 마운트)은 camelCase.
#[derive(Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKey {
    pub id: String,
    pub sha256: String,
    pub service: String,
    #[serde(default)]
    pub write_buckets: Vec<String>,
    #[serde(default)]
    pub read_buckets: Vec<String>,
    #[serde(default)]
    pub admin: bool,
}

#[derive(Clone)]
pub struct KeyRegistry {
    keys: Vec<ApiKey>,
}

impl KeyRegistry {
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let keys = serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Self { keys })
    }

    /// bearer를 해시해 등록된 sha256과 상수시간 비교. 전체 키를 순회(타이밍 누설 방지).
    pub fn authenticate(&self, bearer: &str) -> Option<&ApiKey> {
        let want = hex::encode(Sha256::digest(bearer.as_bytes()));
        let mut found = None;
        for k in &self.keys {
            if bool::from(k.sha256.as_bytes().ct_eq(want.as_bytes())) {
                found = Some(k);
            }
        }
        found
    }
}

impl ApiKey {
    pub fn can_write(&self, b: &str) -> bool {
        self.admin || self.write_buckets.iter().any(|x| x == b)
    }
    pub fn can_read(&self, b: &str) -> bool {
        self.admin || self.read_buckets.iter().any(|x| x == b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha_hex(s: &str) -> String {
        hex::encode(Sha256::digest(s.as_bytes()))
    }

    fn write_keys(json: &str) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn camelcase_fixture_scoped_read_write() {
        // 문서 keys.json 형식(camelCase): writeBuckets/readBuckets
        let json = format!(
            r#"[
              {{"id":"k1","sha256":"{}","service":"page","writeBuckets":["skills"],"readBuckets":["skills","public"]}},
              {{"id":"adm","sha256":"{}","service":"ops","admin":true}}
            ]"#,
            sha_hex("page-token"),
            sha_hex("admin-token")
        );
        let f = write_keys(&json);
        let reg = KeyRegistry::load(f.path()).unwrap();

        let k = reg.authenticate("page-token").unwrap();
        assert_eq!(k.service, "page");
        assert!(k.can_write("skills"));
        assert!(!k.can_write("public")); // read 전용
        assert!(k.can_read("skills"));
        assert!(k.can_read("public"));
        assert!(!k.can_read("secret"));
        assert!(!k.admin);

        let a = reg.authenticate("admin-token").unwrap();
        assert!(a.admin);
        assert!(a.can_write("anything"));
        assert!(a.can_read("anything"));

        assert!(reg.authenticate("wrong-token").is_none());
    }

    #[test]
    fn missing_scopes_default_to_empty() {
        let json = format!(r#"[{{"id":"x","sha256":"{}","service":"x"}}]"#, sha_hex("t"));
        let f = write_keys(&json);
        let reg = KeyRegistry::load(f.path()).unwrap();
        let k = reg.authenticate("t").unwrap();
        assert!(!k.can_write("any"));
        assert!(!k.can_read("any"));
    }

    #[test]
    fn malformed_keys_file_errors() {
        let f = write_keys("not json");
        assert!(KeyRegistry::load(f.path()).is_err());
    }
}
