use crate::{
    transport_api::{ReplyError, ReplyErrorKind},
    types::v0::openapi::{
        actix::server::RestError,
        apis::StatusCode,
        models::{rest_json_error::Kind, RestJsonError},
    },
};

pub mod v0;

impl From<ReplyError> for RestError<RestJsonError> {
    fn from(src: ReplyError) -> Self {
        let details = src.extra.clone();
        let message = src.source.clone();
        let (status, error) = match &src.kind {
            ReplyErrorKind::WithMessage => {
                let error = RestJsonError::new(details, message, Kind::Internal);
                (StatusCode::INTERNAL_SERVER_ERROR, error)
            }
            ReplyErrorKind::DeserializeReq => {
                let error = RestJsonError::new(details, message, Kind::Deserialize);
                (StatusCode::BAD_REQUEST, error)
            }
            ReplyErrorKind::Internal => {
                let error = RestJsonError::new(details, message, Kind::Internal);
                (StatusCode::INTERNAL_SERVER_ERROR, error)
            }
            ReplyErrorKind::Timeout => {
                let error = RestJsonError::new(details, message, Kind::Timeout);
                (StatusCode::REQUEST_TIMEOUT, error)
            }
            ReplyErrorKind::InvalidArgument => {
                let error = RestJsonError::new(details, message, Kind::InvalidArgument);
                (StatusCode::BAD_REQUEST, error)
            }
            ReplyErrorKind::DeadlineExceeded => {
                let error = RestJsonError::new(details, message, Kind::DeadlineExceeded);
                (StatusCode::GATEWAY_TIMEOUT, error)
            }
            ReplyErrorKind::NotFound => {
                let error = RestJsonError::new(details, message, Kind::NotFound);
                (StatusCode::NOT_FOUND, error)
            }
            ReplyErrorKind::AlreadyExists => {
                let error = RestJsonError::new(details, message, Kind::AlreadyExists);
                (StatusCode::UNPROCESSABLE_ENTITY, error)
            }
            ReplyErrorKind::PermissionDenied => {
                let error = RestJsonError::new(details, message, Kind::PermissionDenied);
                (StatusCode::UNAUTHORIZED, error)
            }
            ReplyErrorKind::ResourceExhausted => {
                let error = RestJsonError::new(details, message, Kind::ResourceExhausted);
                (StatusCode::INSUFFICIENT_STORAGE, error)
            }
            ReplyErrorKind::FailedPrecondition => {
                let error = RestJsonError::new(details, message, Kind::FailedPrecondition);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::Aborted => {
                let error = RestJsonError::new(details, message, Kind::Aborted);
                (StatusCode::SERVICE_UNAVAILABLE, error)
            }
            ReplyErrorKind::OutOfRange => {
                let error = RestJsonError::new(details, message, Kind::OutOfRange);
                (StatusCode::RANGE_NOT_SATISFIABLE, error)
            }
            ReplyErrorKind::Unimplemented => {
                let error = RestJsonError::new(details, message, Kind::Unimplemented);
                (StatusCode::NOT_IMPLEMENTED, error)
            }
            ReplyErrorKind::Unavailable => {
                let error = RestJsonError::new(details, message, Kind::Unavailable);
                (StatusCode::SERVICE_UNAVAILABLE, error)
            }
            ReplyErrorKind::Unauthenticated => {
                let error = RestJsonError::new(details, message, Kind::Unauthenticated);
                (StatusCode::UNAUTHORIZED, error)
            }
            ReplyErrorKind::Unauthorized => {
                let error = RestJsonError::new(details, message, Kind::Unauthorized);
                (StatusCode::UNAUTHORIZED, error)
            }
            ReplyErrorKind::Conflict => {
                let error = RestJsonError::new(details, message, Kind::Conflict);
                (StatusCode::CONFLICT, error)
            }
            ReplyErrorKind::FailedPersist => {
                let error = RestJsonError::new(details, message, Kind::FailedPersist);
                (StatusCode::INSUFFICIENT_STORAGE, error)
            }
            ReplyErrorKind::AlreadyShared => {
                let error = RestJsonError::new(details, message, Kind::AlreadyShared);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::NotShared => {
                let error = RestJsonError::new(details, message, Kind::NotShared);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::NotPublished => {
                let error = RestJsonError::new(details, message, Kind::NotPublished);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::AlreadyPublished => {
                let error = RestJsonError::new(details, message, Kind::AlreadyPublished);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::Deleting => {
                let error = RestJsonError::new(details, message, Kind::Deleting);
                (StatusCode::CONFLICT, error)
            }
            ReplyErrorKind::ReplicaCountAchieved => {
                let error = RestJsonError::new(details, message, Kind::FailedPrecondition);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::ReplicaChangeCount => {
                let error = RestJsonError::new(details, message, Kind::FailedPrecondition);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::ReplicaIncrease => {
                let error = RestJsonError::new(details, message, Kind::FailedPrecondition);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::VolumeNoReplicas => {
                let error = RestJsonError::new(details, message, Kind::FailedPrecondition);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::InUse => {
                let error = RestJsonError::new(details, message, Kind::InUse);
                (StatusCode::CONFLICT, error)
            }
            ReplyErrorKind::ReplicaCreateNumber => {
                let error = RestJsonError::new(details, message, Kind::FailedPrecondition);
                (StatusCode::PRECONDITION_FAILED, error)
            }
            ReplyErrorKind::CapacityLimitExceeded => {
                let error = RestJsonError::new(details, message, Kind::CapacityLimitExceeded);
                (StatusCode::INSUFFICIENT_STORAGE, error)
            }
            ReplyErrorKind::NotAcceptable => {
                let error = RestJsonError::new(details, message, Kind::NotAcceptable);
                (StatusCode::NOT_ACCEPTABLE, error)
            }
            ReplyErrorKind::Cancelled => {
                let error = RestJsonError::new(details, message, Kind::Cancelled);
                (StatusCode::GATEWAY_TIMEOUT, error)
            }
        };

        RestError::new(status, error)
    }
}
