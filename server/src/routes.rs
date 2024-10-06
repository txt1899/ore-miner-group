use std::sync::Arc;

use actix_web::{post, web, HttpResponse, Responder};
use shared::interaction::{NextEpoch, Peek, RestfulError, RestfulResponse, Solution, User};
use tracing::trace;

use crate::restful::{IntoRestfulError, RESTful};

#[post("/api/v1/login")]
pub(crate) async fn login(
    web::Json(user): web::Json<User>,
    api: web::Data<Arc<RESTful>>,
) -> Result<impl Responder, RestfulError> {
    trace!("/api/v1/login");
    if user.version.ne(crate::VERSION) {
        Err(RestfulError::VersionMismatch)
    } else {
        let resp = api.user_login(user).await.map_err(|err| err.into_restful_error())?;
        Ok(HttpResponse::Ok().json(RestfulResponse::success(resp)))
    }
}

#[post("/api/v1/epoch")]
pub(crate) async fn epoch(
    web::Json(epoch): web::Json<NextEpoch>,
    api: web::Data<Arc<RESTful>>,
) -> Result<impl Responder, RestfulError> {
    trace!("/api/v1/epoch");
    api.next_epoch(epoch).await.map_err(|err| err.into_restful_error())?;
    Ok(HttpResponse::Ok().json(RestfulResponse::success(())))
}

#[post("/api/v1/solution")]
pub(crate) async fn solution(
    web::Json(data): web::Json<Solution>,
    api: web::Data<Arc<RESTful>>,
) -> Result<impl Responder, RestfulError> {
    trace!("/api/v1/solution");
    let data = api.get_solution(data).await.map_err(|err| err.into_restful_error())?;
    Ok(HttpResponse::Ok().json(RestfulResponse::success(data)))
}

#[post("/api/v1/difficulty")]
pub(crate) async fn difficulty(
    web::Json(data): web::Json<Peek>,
    api: web::Data<Arc<RESTful>>,
) -> Result<impl Responder, RestfulError> {
    trace!("/api/v1/difficulty");
    let resp = api.peek_difficulty(data).await.map_err(|err| err.into_restful_error())?;
    Ok(HttpResponse::Ok().json(RestfulResponse::success(resp)))
}
