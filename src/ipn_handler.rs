use chrono::prelude::*;
use chrono::Duration;
use chrono::SecondsFormat::Secs;
use http_types::auth::BasicAuth;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tide::http::Method;
use tide::{Body, Response, StatusCode};

use crate::azure_function::{AzureFnLogger, AzureFnLoggerExt};
use crate::AppRequest;

#[derive(Debug, Deserialize)]
struct IPNTransationMessage {
    txn_id: String,
    txn_type: Option<String>,
    payment_status: String,
    payer_email: String,
    first_name: String,
    last_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct MailchimpResponse {
    status: String,
    email_address: String,
}

#[derive(Debug, Deserialize)]
struct MailchimpErrorResponse {
    title: String,
}

pub async fn ipn_handler(mut req: AppRequest) -> tide::Result<Response> {
    let mut logger = req
        .ext_mut::<AzureFnLogger>()
        .expect("Must install AzureFnMiddleware")
        .clone();

    if req.method() != Method::Post {
        logger
            .log(format!(
                "Request method was not allowed. Was: {}",
                req.method()
            ))
            .await;
        return Err(tide::Error::from_str(
            StatusCode::MethodNotAllowed,
            StatusCode::MethodNotAllowed.to_string(),
        ));
    }
    logger
        .log("PayPal IPN Notification Event received successfully.".to_string())
        .await;

    let ipn_transaction_message_raw = req.body_string().await?;
    let verification_body = ["cmd=_notify-validate&", &ipn_transaction_message_raw].concat();

    // Must be done after we take the main request body.
    let state = req.state();

    if state.paypal_sandbox {
        logger
            .log("SANDBOX: Using PayPal sandbox environment".to_string())
            .await;
    }

    let mut verify_response = state
        .paypal
        .post("/cgi-bin/webscr")
        .body(verification_body)
        .await?;

    if !verify_response.status().is_success() {
        return Err(tide::Error::from_str(
            StatusCode::InternalServerError,
            format!(
                "PayPal IPN verification failed - status: {}",
                verify_response.status()
            ),
        ));
    }

    let ipn_transaction_message: IPNTransationMessage;
    match serde_qs::from_str(&ipn_transaction_message_raw) {
        Ok(msg) => {
            ipn_transaction_message = msg;
        }
        Err(error) => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Invalid IPN: unparseable IPN: {} - error: {}",
                    ipn_transaction_message_raw, error
                ),
            ));
        }
    }

    let verify_status = verify_response.body_string().await?;
    match verify_status.as_str() {
        "VERIFIED" => {
            logger
                .log(format!(
                    "Verified IPN: IPN message for Transaction ID \"{}\" is verified",
                    ipn_transaction_message.txn_id
                ))
                .await
        }
        "INVALID" => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Invalid IPN: IPN message for Transaction ID \"{}\" is invalid",
                    ipn_transaction_message.txn_id
                ),
            ));
        }
        s => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!("Invalid IPN: Unexpected IPN verify response body: {}", s),
            ));
        }
    }

    if ipn_transaction_message.payment_status != "Completed" {
        logger
            .log(format!(
                "IPN: Payment status was not \"Completed\": {}",
                ipn_transaction_message.payment_status
            ))
            .await;
        return Ok(StatusCode::Ok.into());
    }

    match ipn_transaction_message.txn_type.as_deref() {
        Some("web_accept") => (),        // Ok
        Some("subscr_payment") => (),    // TODO: check amount
        Some("send_money") => (),        // TODO: check amount
        Some("recurring_payment") => (), // TODO: check amount
        Some(txn_type) => {
            return Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!("IPN: txn_type was not acceptible: {}", txn_type),
            ));
        }
        None => {
            return Err(tide::Error::from_str(
                StatusCode::Ok,
                format!("IPN: no transaction type: {}", ipn_transaction_message_raw),
            ));
        }
    }

    logger
        .log(format!("Email: {}", ipn_transaction_message.payer_email))
        .await;

    let hash = md5::compute(&ipn_transaction_message.payer_email.to_lowercase());
    let authz = BasicAuth::new("any", &state.mc_api_key);

    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state
        .mailchimp
        .get(&mc_path)
        .header(authz.name(), authz.value())
        .await?;

    if mailchimp_res.status().is_server_error() {
        let error_body = mailchimp_res.body_string().await?;

        logger
            .log(format!(
                "Mailchimp error: {} -- {}",
                mailchimp_res.status(),
                error_body
            ))
            .await;

        return Ok(Response::builder(mailchimp_res.status())
            .body(error_body)
            .into());
    }

    let status;
    if mailchimp_res.status().is_client_error() {
        status = "pending"
    } else {
        let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;
        logger
            .log(format!(
                "Mailchimp existing status: {}",
                mc_json.status.as_str(),
            ))
            .await;
        status = match mc_json.status.as_str() {
            "subscribed" => "subscribed",
            "unsubscribed" => return Ok(StatusCode::Ok.into()),
            _ => "pending",
        }
    };

    let utc_now: DateTime<Utc> = Utc::now();
    let utc_expires: DateTime<Utc> = Utc::now() + Duration::days(365 * 5 + 1);

    let mc_req = json!({
        "email_address": &ipn_transaction_message.payer_email,
        "merge_fields": {
            "FNAME": ipn_transaction_message.first_name,
            "LNAME": ipn_transaction_message.last_name,
            "JOINED": utc_now.to_rfc3339_opts(Secs, true),
            "EXPIRES": utc_expires.to_rfc3339_opts(Secs, true),
        },
        "status": status,
    });

    let mc_path = format!("3.0/lists/{}/members/{:x}", state.mc_list_id, hash);
    let mut mailchimp_res = state
        .mailchimp
        .put(&mc_path)
        .header(authz.name(), authz.value())
        .body(Body::from_json(&mc_req)?)
        .await?;

    if !mailchimp_res.status().is_success() {
        let error_body = mailchimp_res.body_string().await?;

        Err(tide::Error::from_str(
            mailchimp_res.status(),
            format!("Mailchimp error: {}", error_body),
        ))
    } else {
        let mc_json: MailchimpResponse = mailchimp_res.body_json().await?;
        if mc_json.status == "pending" || mc_json.status == "subscribed" {
            logger
                .log(format!(
                    "Mailchimp: successfully set subscription status \"{}\" for: {}",
                    mc_json.status, mc_json.email_address
                ))
                .await;
            Ok(StatusCode::Ok.into())
        } else {
            Err(tide::Error::from_str(
                StatusCode::InternalServerError,
                format!(
                    "Mailchimp: unsuccessful result: {}",
                    serde_json::to_string(&mc_json)?
                ),
            ))
        }
    }
}
