use aws_lambda_events::event::apigw::ApiGatewayProxyResponse;
use aws_sdk_lambda::{Client as LambdaClient, types::InvocationType, primitives::Blob};
use lambda_runtime::{service_fn, LambdaEvent, Error};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::sync::Arc;
use std::time::Instant;
use reqwest::Client;

#[derive(Serialize, Deserialize)]
struct WorkerInput {
    batch_size: usize,
    track_metrics: bool,
}

async fn invoke_direct(lambda_client: Arc<LambdaClient>, function_name: String, track_metrics: bool) {
    let start = Instant::now();
    let payload = json!({});

    let res = lambda_client
        .invoke()
        .function_name(function_name)
        .invocation_type(InvocationType::RequestResponse)
        .payload(Blob::new(payload.to_string().into_bytes()))
        .send()
        .await;

    let latency = start.elapsed().as_secs_f64() * 1000.0;

    if track_metrics {
        emit_emf_metric("lambda_invoke_latency", latency, res.is_ok());
    }
}

async fn invoke_momento(client: Arc<Client>, track_metrics: bool) {
  let start = Instant::now();

  let url = "https://api.cache.developer-kenny-dev.preprod.a.momentohq.com/functions/fls/calc";
  let payload = json!({
      "a": 10,
      "b": 20,
      "c": "*"
  });

  let res = client
      .post(url)
      .header("authorization", "") // 🔁 Replace with real key if needed
      .json(&payload)
      .send()
      .await;

  let latency = start.elapsed().as_secs_f64() * 1000.0;
  let success = res.as_ref().map(|r| r.status().is_success()).unwrap_or(false);

  if track_metrics {
      emit_emf_metric("momento_latency", latency, success);
  }
}


async fn invoke_http(client: Arc<Client>, url: String, method_name: &str, track_metrics: bool) {
    let start = Instant::now();
    let res = client.get(&url).send().await;

    let latency = start.elapsed().as_secs_f64() * 1000.0;
    let success = res.as_ref().map(|r| r.status().is_success()).unwrap_or(false);

    if track_metrics {
        emit_emf_metric(&format!("{}_latency", method_name), latency, success);
    }
}

fn emit_emf_metric(metric_name: &str, latency_ms: f64, success: bool) {
    let emf = json!({
        "_aws": {
            "Timestamp": chrono::Utc::now().timestamp_millis(),
            "CloudWatchMetrics": [{
                "Namespace": "LambdaLatencyBenchmark",
                "Dimensions": [["Method", "Success"]],
                "Metrics": [{ "Name": metric_name, "Unit": "Milliseconds" }]
            }]
        },
        "Method": metric_name,
        "Success": success,
        metric_name: latency_ms
    });

    println!("{}", serde_json::to_string(&emf).unwrap());
}

async fn handler(event: LambdaEvent<WorkerInput>) -> Result<ApiGatewayProxyResponse, Error> {
    let batch_size = event.payload.batch_size;
    let track_metrics = event.payload.track_metrics;

    let rust_lambda_function_name = env::var("RUST_LAMBDA_FUNCTION_NAME").unwrap_or_default();
    let lambda_url = env::var("RUST_LAMBDA_FUNCTION_URL").unwrap_or_default();
    let http_api_url = env::var("RUST_HTTP_API_ENDPOINT").unwrap_or_default();
    let rest_api_url = env::var("RUST_REST_API_ENDPOINT").unwrap_or_default();

    let aws_config = aws_config::load_from_env().await;
    let lambda_client = Arc::new(LambdaClient::new(&aws_config));
    let http_client = Arc::new(Client::new());

    let mut handles = Vec::new();

    for _ in 0..batch_size {
        let lambda_client = lambda_client.clone();
        let http_client = http_client.clone();

        let rust_lambda_function_name = rust_lambda_function_name.clone();
        let lambda_url = lambda_url.clone();
        let http_api_url = http_api_url.clone();
        let rest_api_url = rest_api_url.clone();

        let handle = tokio::spawn(async move {
            let d = tokio::spawn(invoke_direct(lambda_client.clone(), rust_lambda_function_name.clone(), track_metrics));
            let f = tokio::spawn(invoke_http(http_client.clone(), lambda_url.clone(), "lambda_function_url", track_metrics));
            let h = tokio::spawn(invoke_http(http_client.clone(), http_api_url.clone(), "http_api_endpoint", track_metrics));
            let r = tokio::spawn(invoke_http(http_client.clone(), rest_api_url.clone(), "rest_api_endpoint", track_metrics));
            let momento = tokio::spawn(invoke_momento(http_client.clone(), track_metrics));
            let _ = tokio::try_join!(d, f, h, r, momento);
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await?;
    }

    Ok(ApiGatewayProxyResponse {
        status_code: 200,
        body: Some(aws_lambda_events::encodings::Body::Text("{\"status\": \"completed\"}".to_string())),
        headers: Default::default(),
        multi_value_headers: Default::default(),
        is_base64_encoded: false,
    })
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    lambda_runtime::run(service_fn(handler)).await
}
