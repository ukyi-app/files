use thiserror::Error;

/// 도메인 에러. HTTP 상태/코드 매핑을 보유하되 axum 의존은 M6(`IntoResponse`)에서 추가.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    /// 인자 &'static str이 곧 클라이언트 노출 에러 코드.
    #[error("bad request: {0}")]
    BadRequest(&'static str),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("payload too large")]
    TooLarge,
    #[error("insufficient storage")]
    InsufficientStorage,
    #[error("conflict")]
    Conflict,
    #[error("internal: {0}")]
    Internal(#[from] std::io::Error),
}

impl AppError {
    /// HTTP 상태 코드(u16). axum StatusCode 변환은 M6.
    pub fn status(&self) -> u16 {
        match self {
            AppError::NotFound => 404,
            AppError::BadRequest(_) => 400,
            AppError::Unauthorized => 401,
            AppError::Forbidden => 403,
            AppError::TooLarge => 413,
            AppError::InsufficientStorage => 507,
            AppError::Conflict => 409,
            AppError::Internal(_) => 500,
        }
    }

    /// JSON 바디 `{"error": <code>}`에 노출할 안정적 에러 코드.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::NotFound => "not_found",
            AppError::BadRequest(s) => s,
            AppError::Unauthorized => "unauthorized",
            AppError::Forbidden => "forbidden",
            AppError::TooLarge => "too_large",
            AppError::InsufficientStorage => "insufficient_storage",
            AppError::Conflict => "conflict",
            AppError::Internal(_) => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping() {
        assert_eq!(AppError::NotFound.status(), 404);
        assert_eq!(AppError::BadRequest("invalid_key").status(), 400);
        assert_eq!(AppError::Unauthorized.status(), 401);
        assert_eq!(AppError::Forbidden.status(), 403);
        assert_eq!(AppError::TooLarge.status(), 413);
        assert_eq!(AppError::InsufficientStorage.status(), 507);
        assert_eq!(AppError::Conflict.status(), 409);
        let io = std::io::Error::new(std::io::ErrorKind::Other, "x");
        assert_eq!(AppError::Internal(io).status(), 500);
    }

    #[test]
    fn code_mapping() {
        assert_eq!(AppError::NotFound.code(), "not_found");
        // BadRequest의 &'static str이 곧 코드
        assert_eq!(AppError::BadRequest("invalid_bucket").code(), "invalid_bucket");
        assert_eq!(AppError::Unauthorized.code(), "unauthorized");
        assert_eq!(AppError::Forbidden.code(), "forbidden");
        assert_eq!(AppError::TooLarge.code(), "too_large");
        assert_eq!(AppError::InsufficientStorage.code(), "insufficient_storage");
        assert_eq!(AppError::Conflict.code(), "conflict");
        let io = std::io::Error::new(std::io::ErrorKind::Other, "x");
        assert_eq!(AppError::Internal(io).code(), "internal");
    }
}
