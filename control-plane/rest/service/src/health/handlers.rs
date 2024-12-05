use crate::CachedCoreState;
use actix_web::{get, web::Data, HttpResponse, Responder};

/// Liveness probe check. Failure will result in Pod restart. 200 on success.
#[get("/live")]
async fn liveness(_cached_core_state: Data<CachedCoreState>) -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .insert_header(("X-Content-Type-Options", "nosniff"))
        .body("live")
}

/// Readiness probe check. Failure will result in removal of Container from Kubernetes service
/// target pool. 200 on success, 503 on failure.
#[get("/ready")]
async fn readiness(cached_core_state: Data<CachedCoreState>) -> HttpResponse {
    if cached_core_state.get_or_assume_unavailable().await {
        return HttpResponse::Ok()
            .content_type("text/plain; charset=utf-8")
            .insert_header(("X-Content-Type-Options", "nosniff"))
            .body("ready");
    }

    HttpResponse::ServiceUnavailable()
        .content_type("text/plain; charset=utf-8")
        .insert_header(("X-Content-Type-Options", "nosniff"))
        .body("not ready")
}
