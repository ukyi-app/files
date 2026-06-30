use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub keys_path: PathBuf,
    pub internal_port: u16,
    pub public_port: u16,
    pub max_file_bytes: u64,
    pub min_free_bytes: u64,
    pub gc_grace_secs: u64,
    pub upload_timeout_secs: u64,
    pub public_base_url: String,
}

impl Config {
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let data_dir: PathBuf = get("FILES_DATA_DIR").ok_or("FILES_DATA_DIR required")?.into();
        let keys_path: PathBuf = get("FILES_KEYS_PATH").ok_or("FILES_KEYS_PATH required")?.into();
        let pu16 = |k: &str, d: u16| -> Result<u16, String> {
            get(k)
                .map(|v| v.parse().map_err(|_| format!("{k} invalid")))
                .transpose()
                .map(|o| o.unwrap_or(d))
        };
        let pu64 = |k: &str, d: u64| -> Result<u64, String> {
            get(k)
                .map(|v| v.parse().map_err(|_| format!("{k} invalid")))
                .transpose()
                .map(|o| o.unwrap_or(d))
        };
        Ok(Config {
            data_dir,
            keys_path,
            internal_port: pu16("FILES_INTERNAL_PORT", 8080)?,
            public_port: pu16("FILES_PUBLIC_PORT", 8081)?,
            max_file_bytes: pu64("FILES_MAX_FILE_BYTES", 1024 * 1024 * 1024)?,
            min_free_bytes: pu64("FILES_MIN_FREE_BYTES", 2 * 1024 * 1024 * 1024)?,
            gc_grace_secs: pu64("FILES_GC_GRACE", 3600)?,
            // 업로드 바디 타임아웃(< gc_grace, M8.1에서 불변식 검증)
            upload_timeout_secs: pu64("FILES_UPLOAD_TIMEOUT", 600)?,
            public_base_url: get("FILES_PUBLIC_BASE_URL")
                .unwrap_or_else(|| "https://files.ukyi.app".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_with_required() {
        let env = |k: &str| match k {
            "FILES_DATA_DIR" => Some("/data".into()),
            "FILES_KEYS_PATH" => Some("/etc/files/keys.json".into()),
            _ => None,
        };
        let c = Config::from_env(env).unwrap();
        assert_eq!(c.internal_port, 8080);
        assert_eq!(c.public_port, 8081);
        assert_eq!(c.max_file_bytes, 1024 * 1024 * 1024);
        assert!(c.min_free_bytes > 0);
    }

    #[test]
    fn missing_required_errors() {
        assert!(Config::from_env(|_| None).is_err());
    }
}
