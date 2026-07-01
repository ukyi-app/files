use serde::{Deserialize, Serialize};

/// 키의 커밋 포인터. on-disk `<key>.meta.json`(camelCase)이자 API 응답 본문.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMeta {
    pub content_type: String,
    pub size: u64,
    pub sha256: String,
    pub created_at: String,
    pub uploaded_by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Internal,
}

/// 버킷 메타. on-disk `<bucket>/.bucket.json`(camelCase).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BucketMeta {
    pub visibility: Visibility,
    pub owner: String,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_meta_roundtrip_camel_case() {
        let m = ObjectMeta {
            content_type: "application/zip".into(),
            size: 42,
            sha256: "abc".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            uploaded_by: "page".into(),
        };
        let j = serde_json::to_string(&m).unwrap();
        assert!(j.contains("\"contentType\""), "expected camelCase: {j}");
        assert!(j.contains("\"createdAt\""));
        assert!(j.contains("\"uploadedBy\""));
        let back: ObjectMeta = serde_json::from_str(&j).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn visibility_lowercase() {
        assert_eq!(serde_json::to_string(&Visibility::Public).unwrap(), "\"public\"");
        let v: Visibility = serde_json::from_str("\"internal\"").unwrap();
        assert_eq!(v, Visibility::Internal);
    }

    #[test]
    fn bucket_meta_roundtrip_camel_case() {
        let b = BucketMeta {
            visibility: Visibility::Public,
            owner: "page".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&b).unwrap();
        assert!(j.contains("\"createdAt\""), "expected camelCase: {j}");
        let back: BucketMeta = serde_json::from_str(&j).unwrap();
        assert_eq!(back.owner, "page");
        assert_eq!(back.visibility, Visibility::Public);
    }
}
