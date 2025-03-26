use aws_lambda_events::event::apigw::ApiGatewayProxyResponse;
use aws_sdk_lambda::{Client as LambdaClient, types::InvocationType, primitives::Blob};
use lambda_runtime::{service_fn, LambdaEvent, Error};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use tokio::time::{sleep, Duration};
use tokio::sync::Semaphore;
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct DriverInput {
    invocations_per_second: usize,
    run_duration_seconds: usize,
    batch_size: usize,
}

async fn invoke_worker(lambda_client: &LambdaClient, function_name: &str, batch_size: usize, track_metrics: bool) {
    let payload = json!({
        "batch_size": batch_size,
        "track_metrics": track_metrics
    });

    let result = lambda_client
        .invoke()
        .function_name(function_name)
        .invocation_type(InvocationType::Event)
        .payload(Blob::new(payload.to_string().into_bytes()))
        .send()
        .await;

    if let Err(e) = result {
        eprintln!("Worker invocation failed: {}", e);
    }
}

async fn warm_up(lambda_client: &LambdaClient, function_name: &str, batch_size: usize, count: usize) {
    for _ in 0..count {
        invoke_worker(lambda_client, function_name, batch_size, false).await;
    }
}

async fn handler(event: LambdaEvent<DriverInput>) -> Result<ApiGatewayProxyResponse, Error> {
    let input = event.payload;
    let worker_lambda_function_name = env::var("WORKER_LAMBDA_FUNCTION_NAME").unwrap_or_default();

    let aws_config = aws_config::load_from_env().await;
    let lambda_client = Arc::new(LambdaClient::new(&aws_config));
    let concurrency_limit = Arc::new(Semaphore::new(input.invocations_per_second));

    let invocations_per_second = input.invocations_per_second;
    let run_duration_seconds = input.run_duration_seconds;
    let batch_size = input.batch_size;

    // Warm-up: prevent cold starts from polluting metrics
    warm_up(&lambda_client, &worker_lambda_function_name, batch_size, 10).await;

    for _ in 0..run_duration_seconds {
        let mut handles = Vec::with_capacity(invocations_per_second);

        for _ in 0..invocations_per_second {
            let permit = concurrency_limit.clone().acquire_owned().await.unwrap();
            let client = lambda_client.clone();
            let fn_name = worker_lambda_function_name.clone();

            let handle = tokio::spawn(async move {
                invoke_worker(&client, &fn_name, batch_size, true).await;
                drop(permit);
            });

            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        // Maintain 1-second pacing between rounds
        sleep(Duration::from_secs(1)).await;
    }

    Ok(ApiGatewayProxyResponse {
        status_code: 200,
        body: Some(aws_lambda_events::encodings::Body::Text(
            "{\"status\": \"driver_completed\"}".to_string(),
        )),
        headers: Default::default(),
        multi_value_headers: Default::default(),
        is_base64_encoded: false,
    })
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    lambda_runtime::run(service_fn(handler)).await
}
