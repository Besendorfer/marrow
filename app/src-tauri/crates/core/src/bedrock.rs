use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::error::ProvideErrorMetadata;
use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message};
use aws_sdk_bedrockruntime::Client;

pub struct BedrockClient {
    client: Client,
}

impl BedrockClient {
    pub async fn new(region: &str, profile: &str) -> Result<Self, String> {
        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()));

        if !profile.is_empty() {
            loader = loader.profile_name(profile);
        }

        let config = loader.load().await;
        let client = Client::new(&config);
        Ok(Self { client })
    }

    pub async fn invoke_model(
        &self,
        model_arn: &str,
        prompt: &str,
    ) -> Result<String, String> {
        let message = Message::builder()
            .role(ConversationRole::User)
            .content(ContentBlock::Text(prompt.to_string()))
            .build()
            .map_err(|e| format!("Failed to build message: {}", e))?;

        let response = self
            .client
            .converse()
            .model_id(model_arn)
            .messages(message)
            .send()
            .await
            .map_err(|e| {
                // Extract the detailed error from the SDK error chain
                let msg = e
                    .as_service_error()
                    .map(|se| format!("{}: {}", se.code().unwrap_or("Unknown"), se.message().unwrap_or("no details")))
                    .unwrap_or_else(|| format!("{}", e));
                format!("Bedrock API error: {}. Make sure you are logged in: aws sso login --profile claude-code-bedrock", msg)
            })?;

        let output = response
            .output()
            .ok_or_else(|| "No output from Bedrock".to_string())?;

        match output {
            aws_sdk_bedrockruntime::types::ConverseOutput::Message(msg) => {
                for block in msg.content() {
                    if let ContentBlock::Text(text) = block {
                        return Ok(text.clone());
                    }
                }
                Err("No text content in Bedrock response".to_string())
            }
            _ => Err("Unexpected Bedrock response type".to_string()),
        }
    }
}

/// Extract the AWS region from a Bedrock model ARN.
/// ARN format: arn:aws:bedrock:REGION:ACCOUNT:...
pub fn region_from_arn(arn: &str) -> Result<String, String> {
    let parts: Vec<&str> = arn.split(':').collect();
    if parts.len() >= 4 && parts[0] == "arn" {
        Ok(parts[3].to_string())
    } else {
        Err(format!("Could not extract region from model ARN: {}", arn))
    }
}
