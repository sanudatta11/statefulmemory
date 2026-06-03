//! Convert `memlayer_core::Error` → `tonic::Status`.
//!
//! Spec sections: FR12, EH-1..3.
//!
//! Wire shape (PRD §6.2):
//!   - status code from `ErrorKind`
//!   - status message = error's display string
//!   - status details (single google.rpc.ErrorInfo) carrying
//!     `{"code": "<STABLE_CODE>", "message": "...", "details": {...}}`
//!     as JSON bytes inside a `google.protobuf.Any`.

use memlayer_core::error::{Error, ErrorKind};
use tonic::Status;

pub fn to_status(err: Error) -> Status {
    let code = err.code();
    let kind = err.kind();
    let message = err.to_string();

    let detail_json = serde_json::json!({
        "code": code,
        "message": message,
        "details": serde_json::Map::new(),
    });

    let any = prost_types::Any {
        type_url: "type.googleapis.com/google.rpc.ErrorInfo".to_string(),
        value: detail_json.to_string().into_bytes(),
    };

    let mut status = match kind {
        ErrorKind::InvalidArgument => Status::invalid_argument(message),
        ErrorKind::NotFound => Status::not_found(message),
        ErrorKind::AlreadyExists => Status::already_exists(message),
        ErrorKind::FailedPrecondition => Status::failed_precondition(message),
        ErrorKind::Unavailable => Status::unavailable(message),
        ErrorKind::ResourceExhausted => Status::resource_exhausted(message),
        ErrorKind::PermissionDenied => Status::permission_denied(message),
        ErrorKind::Unauthenticated => Status::unauthenticated(message),
        ErrorKind::Internal => Status::internal(message),
    };

    // Pack the ErrorInfo into details. Tonic exposes details via metadata.
    let bytes = prost::Message::encode_to_vec(&any);
    if let Ok(value) = tonic::metadata::MetadataValue::try_from(hex::encode(&bytes)) {
        status.metadata_mut().insert("memlayer-error-info-bin", value);
    }
    status
}

/// Helper: short-circuit Result<T, memlayer::Error> → Result<T, tonic::Status>.
pub fn map<T>(r: memlayer_core::Result<T>) -> Result<T, Status> {
    r.map_err(to_status)
}
