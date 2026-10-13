use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rajeshdb_docs::DocError;
use serde_json::json;

pub struct PErr {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl PErr {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> PErr {
        PErr { status, code, message: message.into() }
    }
    pub fn bad(msg: impl Into<String>) -> PErr {
        PErr::new(StatusCode::BAD_REQUEST, "invalid_argument", msg)
    }
    pub fn unauthorized(msg: impl Into<String>) -> PErr {
        PErr::new(StatusCode::UNAUTHORIZED, "unauthorized", msg)
    }
    pub fn forbidden() -> PErr {
        PErr::new(StatusCode::FORBIDDEN, "forbidden", "you do not have permission to do that")
    }
    pub fn not_found(msg: impl Into<String>) -> PErr {
        PErr::new(StatusCode::NOT_FOUND, "not_found", msg)
    }
    pub fn conflict(msg: impl Into<String>) -> PErr {
        PErr::new(StatusCode::CONFLICT, "already_exists", msg)
    }
    pub fn rate() -> PErr {
        PErr::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many requests, slow down")
    }
    pub fn internal(msg: &str) -> PErr {
        PErr::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", msg)
    }
}

impl IntoResponse for PErr {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": {"code": self.code, "message": self.message}}))).into_response()
    }
}

impl From<DocError> for PErr {
    fn from(e: DocError) -> PErr {
        match e {
            DocError::InvalidArgument(m) => PErr::new(StatusCode::BAD_REQUEST, "invalid_argument", m),
            DocError::NotFound(m) => PErr::not_found(m),
            DocError::AlreadyExists(m) => PErr::conflict(m),
            DocError::SchemaViolation(m) => PErr::new(StatusCode::UNPROCESSABLE_ENTITY, "schema_violation", m),
            DocError::Io(err) => {
                println!("{}", json!({"level": "error", "msg": "storage error", "detail": err.to_string()}));
                PErr::internal("internal storage error")
            }
        }
    }
}
