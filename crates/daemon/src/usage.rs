use std::path::Path;

use api::providers::{UsageBalance, UsageView, UsageWindow};
use serde_json::Value;

async fn read(path: &Path) -> Result<Value, String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| "Connect this provider in Providers.".to_owned())?;
    serde_json::from_slice(&bytes).map_err(|_| "Provider credentials could not be read.".to_owned())
}

async fn response(request: reqwest::RequestBuilder) -> Result<Value, String> {
    let mut response = request
        .send()
        .await
        .map_err(|_| "Usage service could not be reached.".to_owned())?;
    if !response.status().is_success() {
        return Err(if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            "Provider credentials expired. Reconnect in Providers.".into()
        } else {
            format!(
                "Usage service returned HTTP {}.",
                response.status().as_u16()
            )
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Usage response could not be read.".to_owned())?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("Usage response is too large.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Usage service returned invalid JSON.".into())
}

pub(crate) async fn fetch(
    client: &reqwest::Client,
    provider: &str,
    keys: Option<&Path>,
    auth: &Path,
    home: &Path,
    claude: &Path,
) -> Result<UsageView, String> {
    let data = match provider {
        "agy" => {
            let credentials = read(&home.join("antigravity-acp/acp_token.json")).await?;
            let mut form = std::collections::HashMap::new();
            for field in ["client_id", "client_secret", "refresh_token"] {
                form.insert(
                    field,
                    credentials[field]
                        .as_str()
                        .ok_or("Antigravity credentials are incomplete.")?,
                );
            }
            form.insert("grant_type", "refresh_token");
            // The token file's token_uri is not trusted as a destination for credentials.
            let token = response(
                client
                    .post("https://oauth2.googleapis.com/token")
                    .form(&form),
            )
            .await?;
            let access = token["access_token"]
                .as_str()
                .ok_or("Antigravity did not return an access token.")?;
            let summary_fut = client
                .post("https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary")
                .bearer_auth(access)
                .header("User-Agent", "antigravity/cli/1.0 (aidev_client; os_type=linux; arch=amd64; auth_method=consumer)")
                .json(&serde_json::json!({"project": credentials["project_id"]}));
            let quota_fut = client
                .post("https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota")
                .bearer_auth(access)
                .header("User-Agent", "antigravity/cli/1.0 (aidev_client; os_type=linux; arch=amd64; auth_method=consumer)")
                .json(&serde_json::json!({"project": credentials["project_id"]}));
            let (summary, quota) = tokio::join!(response(summary_fut), response(quota_fut));
            serde_json::json!({
                "summary": summary.ok(),
                "quota": quota.ok(),
            })
        }
        "openai-codex" => {
            let credentials = read(auth).await?;
            let credentials = &credentials["openai-codex"];
            let access = credentials["access"]
                .as_str()
                .ok_or("Connect ChatGPT in Providers.")?;
            let mut request = client
                .get("https://chatgpt.com/backend-api/wham/usage")
                .bearer_auth(access)
                .header("Accept", "application/json");
            if let Some(account) = credentials["accountId"].as_str() {
                request = request.header("ChatGPT-Account-Id", account);
            }
            response(request).await?
        }
        "opencode-go" | "openrouter" | "deepseek" => {
            let keys = read(keys.ok_or("No provider keys are configured.")?).await?;
            let key = keys[provider]
                .as_str()
                .ok_or("Connect this provider in Providers.")?;
            let url = match provider {
                "opencode-go" => "https://opencode.ai/zen/go/v1/usage",
                "openrouter" => "https://openrouter.ai/api/v1/credits",
                _ => "https://api.deepseek.com/user/balance",
            };
            response(
                client
                    .get(url)
                    .bearer_auth(key)
                    .header("Accept", "application/json"),
            )
            .await?
        }
        "claude" => {
            let credentials = read(&claude.join(".credentials.json")).await?;
            let access = credentials["claudeAiOauth"]["accessToken"]
                .as_str()
                .ok_or("Claude is not signed in. Connect it in Providers.")?;
            response(
                client
                    .get("https://api.anthropic.com/api/oauth/usage")
                    .bearer_auth(access)
                    .header("anthropic-beta", "oauth-2025-04-20")
                    .header("anthropic-version", "2023-06-01")
                    .header("User-Agent", "claude-cli/2.1.319")
                    .header("Accept", "application/json"),
            )
            .await?
        }
        _ => return Err("This provider has no supported usage endpoint.".into()),
    };
    parse(provider, &data)
}

fn number(value: &Value) -> Result<f64, String> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .ok_or_else(|| "Usage response has no valid amount.".into())
}

fn reset(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}

fn window(label: String, percent: &Value, resets_at: &Value) -> Result<UsageWindow, String> {
    Ok(UsageWindow {
        label,
        used_percent: number(percent)?.clamp(0.0, 100.0),
        resets_at: reset(resets_at),
    })
}

fn parse_agy(data: &Value, view: &mut UsageView) -> Result<(), String> {
    let summary_groups = data
        .get("summary")
        .and_then(|s| s.get("groups"))
        .or_else(|| data.get("groups"));
    if let Some(groups) = summary_groups.and_then(Value::as_array) {
        for group in groups {
            let group_name = group["displayName"].as_str().unwrap_or("Claude & GPT");
            if let Some(buckets) = group["buckets"].as_array() {
                for bucket in buckets {
                    if let Ok(rem) = number(&bucket["remainingFraction"]) {
                        let used = ((1.0 - rem) * 100.0 * 100.0).round() / 100.0;
                        let window_label = match bucket["window"].as_str() {
                            Some("5h") => "5-hour",
                            Some("weekly") => "Weekly",
                            Some(other) => other,
                            None => "Quota",
                        };
                        let label = format!("{group_name} · {window_label}");
                        view.windows.push(window(
                            label,
                            &serde_json::json!(used),
                            &bucket["resetTime"],
                        )?);
                    }
                }
            }
        }
    }
    let quota_buckets = data
        .get("quota")
        .and_then(|q| q.get("buckets"))
        .or_else(|| data.get("buckets"));
    if let Some(buckets) = quota_buckets.and_then(Value::as_array) {
        let mut gemini_added = false;
        let mut argon_added = false;
        for bucket in buckets {
            let model_id = bucket["modelId"].as_str().unwrap_or_default();
            if !gemini_added && (model_id.starts_with("gemini") || model_id.contains("flash")) {
                if let Ok(rem) = number(&bucket["remainingFraction"]) {
                    let used = ((1.0 - rem) * 100.0 * 100.0).round() / 100.0;
                    view.windows.push(window(
                        "Gemini · 5-hour".into(),
                        &serde_json::json!(used),
                        &bucket["resetTime"],
                    )?);
                    gemini_added = true;
                }
            } else if !argon_added && (model_id.starts_with("chat_") || model_id.contains("argon"))
            {
                if let Ok(rem) = number(&bucket["remainingFraction"]) {
                    let used = ((1.0 - rem) * 100.0 * 100.0).round() / 100.0;
                    view.windows.push(window(
                        "Argon · Quota".into(),
                        &serde_json::json!(used),
                        &bucket["resetTime"],
                    )?);
                    argon_added = true;
                }
            }
        }
    }
    Ok(())
}

fn parse(provider: &str, data: &Value) -> Result<UsageView, String> {
    let mut view = UsageView::default();
    match provider {
        "openai-codex" => {
            for (field, label) in [("primary_window", "5-hour"), ("secondary_window", "Weekly")] {
                let item = &data["rate_limit"][field];
                view.windows.push(window(
                    label.into(),
                    &item["used_percent"],
                    &item["reset_at"],
                )?);
            }
        }
        "opencode-go" => {
            for (field, label) in [
                ("rolling", "5-hour"),
                ("weekly", "Weekly"),
                ("monthly", "Monthly"),
            ] {
                let item = &data["usage"][field];
                if !item.is_null() {
                    view.windows
                        .push(window(label.into(), &item["percent"], &item["resetsAt"])?);
                }
            }
        }
        "agy" => parse_agy(data, &mut view)?,
        "openrouter" => {
            let total = number(&data["data"]["total_credits"])?;
            let used = number(&data["data"]["total_usage"])?;
            view.balance = Some(UsageBalance {
                currency: "USD".into(),
                remaining: (total - used).max(0.0),
            });
        }
        "deepseek" => {
            let balances = data["balance_infos"]
                .as_array()
                .ok_or("DeepSeek returned no balances.")?;
            let item = balances
                .iter()
                .find(|item| item["currency"] == "USD")
                .or_else(|| balances.first())
                .ok_or("DeepSeek returned no balances.")?;
            view.balance = Some(UsageBalance {
                currency: item["currency"]
                    .as_str()
                    .ok_or("DeepSeek returned no currency.")?
                    .into(),
                remaining: number(&item["total_balance"])?,
            });
        }
        "claude" => {
            for (field, label) in [("five_hour", "5-hour"), ("seven_day", "Weekly")] {
                let item = &data[field];
                if !item.is_null() {
                    view.windows.push(window(
                        label.into(),
                        &item["utilization"],
                        &item["resets_at"],
                    )?);
                }
            }
        }
        _ => return Err("This provider has no supported usage endpoint.".into()),
    }
    if view.windows.is_empty() && view.balance.is_none() {
        return Err("Provider returned no usage windows.".into());
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn windows_validate_and_clamp_provider_values() {
        let parsed = parse("openai-codex", &json!({"rate_limit": {"primary_window": {"used_percent": 120, "reset_at": 123}, "secondary_window": {"used_percent": -1}}})).unwrap();
        assert_eq!(parsed.windows[0].used_percent, 100.0);
        assert_eq!(parsed.windows[1].used_percent, 0.0);
        assert!(parse("openai-codex", &json!({})).is_err());
    }

    #[test]
    fn credits_allow_zero_balance_but_reject_missing_values() {
        let parsed = parse(
            "openrouter",
            &json!({"data": {"total_credits": 5, "total_usage": 5}}),
        )
        .unwrap();
        assert_eq!(parsed.balance.unwrap().remaining, 0.0);
        assert!(parse("openrouter", &json!({"data": {}})).is_err());
    }

    #[test]
    fn agy_parses_all_three_pools() {
        let data = json!({
            "summary": {
                "groups": [
                    {
                        "displayName": "Claude and GPT models",
                        "buckets": [
                            {
                                "window": "5h",
                                "remainingFraction": 1.0,
                                "resetTime": "2026-11-09T05:00:00Z"
                            },
                            {
                                "window": "weekly",
                                "remainingFraction": 0.5,
                                "resetTime": "2026-11-12T00:00:00Z"
                            }
                        ]
                    }
                ]
            },
            "quota": {
                "buckets": [
                    {
                        "modelId": "gemini-2.5-pro",
                        "remainingFraction": 0.7,
                        "resetTime": "2026-11-09T02:00:00Z"
                    },
                    {
                        "modelId": "chat_20706",
                        "remainingFraction": 1.0
                    }
                ]
            }
        });
        let parsed = parse("agy", &data).unwrap();
        assert_eq!(parsed.windows.len(), 4);
        assert_eq!(parsed.windows[0].label, "Claude and GPT models · 5-hour");
        assert_eq!(parsed.windows[1].label, "Claude and GPT models · Weekly");
        assert_eq!(parsed.windows[2].label, "Gemini · 5-hour");
        assert_eq!(parsed.windows[2].used_percent, 30.0);
        assert_eq!(parsed.windows[3].label, "Argon · Quota");
        assert_eq!(parsed.windows[3].used_percent, 0.0);
    }
}
