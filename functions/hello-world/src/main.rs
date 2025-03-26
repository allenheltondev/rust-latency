use lambda_runtime::{service_fn, LambdaEvent, Error};
use serde_json::{json, Value};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let func = service_fn(my_handler);
    lambda_runtime::run(func).await?;
    Ok(())
}

async fn my_handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
    let response = json!({
        "statusCode": 200,
        "headers": {
            "Content-Type": "application/json"
        },
        "body": json!({
            "message": "Hello, world!"
        }).to_string()
    });

    Ok(response)
}
