use actix_web::{web, Error, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use rusqlite::params;
use serde_json::Value;
use uuid::Uuid;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::db_pool;
use crate::app_state::{AppState};
use crate::rate_limit::{check_rate_limit};

#[derive(Deserialize)]
pub struct IngestEventRequest {
    session_id: String,
    event_name: String,
    time: u64,
    params: Value,
}

#[derive(Serialize)]
struct CreateSessionResponse {
    session_id: String
}

#[derive(Serialize)]
struct Event {
    id: i64,
    event_name: String,
    time: u64,
    params: Value,
}

#[derive(Serialize)]
struct SessionInfo {
    session_id: String,
    start_date: u64,
}

pub async fn create_session(req: HttpRequest, data: web::Data<AppState>) -> impl Responder {
    // Rate limiting per IP address
    let ip = req
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !check_rate_limit(&data, &ip, data.config.create_session_cost) {
        return HttpResponse::TooManyRequests().body("Rate limit exceeded");
    }

    let session_id = Uuid::new_v4().to_string();
    let start_date = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    db_pool::with_connection(|conn| {
        conn.execute(
            "INSERT INTO sessions (session_id, start_date, ip_address) VALUES (?1, ?2, ?3)",
            params![session_id, start_date, ip],
        )
        .unwrap();
    });

    HttpResponse::Ok().json(CreateSessionResponse {
        session_id,
    })
}

pub async fn ingest_event(
    req: HttpRequest,
    data: web::Data<AppState>,
    payload: web::Json<IngestEventRequest>,
) -> impl Responder {
    // Rate limiting per IP address
    let ip = req
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    if !check_rate_limit(&data, &ip, data.config.ingest_event_cost) {
        return HttpResponse::TooManyRequests().body("Rate limit exceeded");
    }

    db_pool::with_connection(|conn| {
        conn.execute(
            "INSERT INTO events (session_id, event_name, time, ip_address, params) VALUES (?1, ?2, ?3, ?4, json(?5))",
            params![
                payload.session_id,
                payload.event_name,
                payload.time,
                ip,
                payload.params.to_string()
            ],
        )
        .unwrap();
    });

    HttpResponse::Ok().body("Event ingested")
}

pub async fn get_events(
    req: HttpRequest,
    data: web::Data<AppState>,
    path: web::Path<String>,
) -> Result<HttpResponse, Error> {
    // Check for shared secret
    let secret = req.headers().get("X-Secret-Key");
    if secret.is_none() || secret.unwrap().to_str().unwrap() != data.config.secret_key {
        return Ok(HttpResponse::Unauthorized().body("Invalid secret key"));
    }

    let session_id = path.into_inner();

    let events = db_pool::with_connection(|conn| {
        let mut stmt = conn
            .prepare_cached(
                "SELECT id, event_name, time, params FROM events WHERE session_id = ?1 ORDER BY time ASC",
            )
            .unwrap();

        let events_iter = stmt
            .query_map(params![session_id], |row| {
                let params_str: String = row.get(3)?;
                Ok(Event {
                    id: row.get(0)?,
                    event_name: row.get(1)?,
                    time: row.get(2)?,
                    params: serde_json::from_str(&params_str).unwrap_or(Value::Null),
                })
            })
            .unwrap();

        events_iter.map(|event| event.unwrap()).collect::<Vec<Event>>()
    });

    Ok(HttpResponse::Ok().json(events))
}

pub async fn get_sessions(req: HttpRequest, data: web::Data<AppState>) -> Result<HttpResponse, Error> {
    // Check for shared secret
    let secret = req.headers().get("X-Secret-Key");
    if secret.is_none() || secret.unwrap().to_str().unwrap() != data.config.secret_key {
        return Ok(HttpResponse::Unauthorized().body("Invalid secret key"));
    }

    let sessions = db_pool::with_connection(|conn| {
        let mut stmt = conn
            .prepare_cached("SELECT session_id, start_date FROM sessions")
            .unwrap();

        let sessions_iter = stmt
            .query_map([], |row| {
                Ok(SessionInfo {
                    session_id: row.get(0)?,
                    start_date: row.get(1)?,
                })
            })
            .unwrap();

        sessions_iter
            .map(|session| session.unwrap())
            .collect::<Vec<SessionInfo>>()
    });

    Ok(HttpResponse::Ok().json(sessions))
}
