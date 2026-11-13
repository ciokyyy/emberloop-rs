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

const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// The Account's access token. Sessions run on the long-lived `oauth-token`, which has no
/// `user:profile` scope, so the usage call needs the sign-in credential; nothing else refreshes
/// it, so it is refreshed here once it is within a minute of expiry and the rotated pair is saved.
async fn claude_access_token(client: &reqwest::Client, file: &Path) -> Result<String, String> {
    let mut credentials = read(file).await?;
    let oauth = &credentials["claudeAiOauth"];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let access = oauth["accessToken"]
        .as_str()
        .ok_or("Claude is not signed in. Connect it in Providers.")?
        .to_owned();
    if oauth["expiresAt"]
        .as_u64()
        .is_none_or(|at| at > now + 60_000)
    {
        return Ok(access);
    }
    let refresh = oauth["refreshToken"]
        .as_str()
        .ok_or("Provider credentials expired. Reconnect in Providers.")?
        .to_owned();
    let reply = response(client.post(CLAUDE_TOKEN_URL).json(&serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": CLAUDE_CLIENT_ID,
    })))
    .await
    .map_err(|_| "Provider credentials expired. Reconnect in Providers.".to_owned())?;
    let new_access = reply["access_token"]
        .as_str()
        .ok_or("Provider credentials expired. Reconnect in Providers.")?
        .to_owned();
    let entry = &mut credentials["claudeAiOauth"];
    entry["accessToken"] = new_access.clone().into();
    if let Some(next) = reply["refresh_token"].as_str() {
        entry["refreshToken"] = next.into();
    }
    let lifetime = reply["expires_in"].as_u64().unwrap_or(3600);
    entry["expiresAt"] = (now + lifetime * 1000).into();
    // The refresh token rotates: a lost write would strand the sign-in, so write through a temp file.
    let bytes = serde_json::to_vec(&credentials).map_err(|e| e.to_string())?;
    let temp = file.with_extension("json.tmp");
    {
        use tokio::io::AsyncWriteExt;
        let mut out = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)
            .await
            .map_err(|e| e.to_string())?;
        out.write_all(&bytes).await.map_err(|e| e.to_string())?;
    }
    tokio::fs::rename(&temp, file)
        .await
        .map_err(|e| e.to_string())?;
    Ok(new_access)
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
            let file = claude.join(".credentials.json");
            let access = claude_access_token(client, &file).await?;
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

fn codex_window_label(seconds: Option<u64>) -> String {
    match seconds {
        Some(18_000) => "5-hour".into(),
        Some(604_800) => "Weekly".into(),
        Some(2_592_000) => "Monthly".into(),
        Some(86_400) => "Daily".into(),
        Some(seconds) if seconds % 86_400 == 0 => format!("{} day", seconds / 86_400),
        Some(seconds) if seconds % 3_600 == 0 => format!("{} hour", seconds / 3_600),
        _ => "Window".into(),
    }
}

fn codex_windows(view: &mut UsageView, limit: &Value, prefix: Option<&str>) -> Result<(), String> {
    for field in ["primary_window", "secondary_window"] {
        let item = &limit[field];
        if item.is_null() {
            continue;
        }
        let label = codex_window_label(item["limit_window_seconds"].as_u64());
        let label = match prefix {
            Some(prefix) => format!("{prefix} · {label}"),
            None => label,
        };
        view.windows
            .push(window(label, &item["used_percent"], &item["reset_at"])?);
    }
    Ok(())
}

fn additional_codex_windows(view: &mut UsageView, limits: &Value) -> Result<(), String> {
    for item in limits.as_array().into_iter().flatten() {
        let limit = item
            .get("rate_limit")
            .filter(|limit| limit.is_object())
            .unwrap_or(item);
        let prefix = item["limit_name"]
            .as_str()
            .or_else(|| item["metered_feature"].as_str())
            .unwrap_or("Additional");
        codex_windows(view, limit, Some(prefix))?;
    }
    Ok(())
}

fn parse(provider: &str, data: &Value) -> Result<UsageView, String> {
    let mut view = UsageView::default();
    match provider {
        "openai-codex" => {
            codex_windows(&mut view, &data["rate_limit"], None)?;
            additional_codex_windows(&mut view, &data["additional_rate_limits"])?;
            codex_windows(
                &mut view,
                &data["code_review_rate_limit"],
                Some("Code review"),
            )?;
            if let Ok(balance) = number(&data["credits"]["balance"]) {
                view.balance = Some(UsageBalance {
                    currency: "USD".into(),
                    remaining: balance,
                });
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
        "claude" => parse_claude(data, &mut view),
        _ => return Err("This provider has no supported usage endpoint.".into()),
    }
    if view.windows.is_empty() && view.balance.is_none() {
        return Err("Provider returned no usage windows.".into());
    }
    Ok(view)
}

fn claude_label(limit: &Value) -> String {
    match limit["kind"].as_str() {
        Some("session") => "5-hour".into(),
        Some("weekly_all") => "Weekly".into(),
        Some("weekly_scoped") => limit["scope"]["model"]["display_name"]
            .as_str()
            .map_or_else(
                || "Scoped · Weekly".into(),
                |name| format!("{name} · Weekly"),
            ),
        Some(kind) => kind.to_string(),
        None => "Limit".into(),
    }
}

fn parse_claude(data: &Value, view: &mut UsageView) {
    for limit in data["limits"].as_array().into_iter().flatten() {
        if let Ok(parsed) = window(claude_label(limit), &limit["percent"], &limit["resets_at"]) {
            view.windows.push(parsed);
        }
    }
    if view.windows.is_empty() {
        for (field, label) in [
            ("five_hour", "5-hour"),
            ("seven_day", "Weekly"),
            ("seven_day_oauth_apps", "Weekly · OAuth apps"),
            ("seven_day_opus", "Weekly · Opus"),
            ("seven_day_sonnet", "Weekly · Sonnet"),
            ("seven_day_cowork", "Weekly · Cowork"),
        ] {
            let item = &data[field];
            if !item.is_null() {
                if let Ok(parsed) = window(label.into(), &item["utilization"], &item["resets_at"]) {
                    view.windows.push(parsed);
                }
            }
        }
    }
    if data["extra_usage"]["is_enabled"] == true {
        if let Ok(parsed) = window(
            "Extra usage".into(),
            &data["extra_usage"]["utilization"],
            &Value::Null,
        ) {
            view.windows.push(parsed);
        }
    }
    if data["spend"]["enabled"] == true {
        if let Ok(parsed) = window("Spend".into(), &data["spend"]["percent"], &Value::Null) {
            view.windows.push(parsed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn a_live_claude_token_is_used_as_is_and_an_expired_one_without_refresh_fails() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".credentials.json");
        let far = u64::MAX / 2;
        std::fs::write(
            &file,
            json!({"claudeAiOauth": {"accessToken": "a", "expiresAt": far}}).to_string(),
        )
        .unwrap();
        let client = reqwest::Client::new();
        assert_eq!(claude_access_token(&client, &file).await.unwrap(), "a");
        std::fs::write(
            &file,
            json!({"claudeAiOauth": {"accessToken": "a", "expiresAt": 1}}).to_string(),
        )
        .unwrap();
        let error = claude_access_token(&client, &file).await.unwrap_err();
        assert!(error.contains("Reconnect"), "{error}");
    }

    #[test]
    fn windows_validate_and_clamp_provider_values() {
        let parsed = parse("openai-codex", &json!({"rate_limit": {"primary_window": {"used_percent": 120, "reset_at": 123}, "secondary_window": {"used_percent": -1}}})).unwrap();
        assert_eq!(parsed.windows[0].used_percent, 100.0);
        assert_eq!(parsed.windows[1].used_percent, 0.0);
        assert!(parse("openai-codex", &json!({})).is_err());
    }

    #[test]
    fn codex_accepts_a_missing_secondary_window() {
        let parsed = parse(
            "openai-codex",
            &json!({"rate_limit": {
                "primary_window": {
                    "used_percent": 0,
                    "limit_window_seconds": 2_592_000,
                    "reset_at": 1
                },
                "secondary_window": null
            }}),
        )
        .unwrap();
        assert_eq!(parsed.windows.len(), 1);
        assert_eq!(parsed.windows[0].label, "Monthly");
    }

    #[test]
    fn codex_reads_additional_and_review_windows() {
        let parsed = parse(
            "openai-codex",
            &json!({
                "rate_limit": {"primary_window": {"used_percent": 5, "limit_window_seconds": 18_000}},
                "additional_rate_limits": [
                    {"limit_name": "GPT-5", "rate_limit": {
                        "primary_window": {"used_percent": 10, "limit_window_seconds": 604_800}
                    }}
                ],
                "code_review_rate_limit": {
                    "primary_window": {"used_percent": 2, "limit_window_seconds": 86_400}
                }
            }),
        )
        .unwrap();
        let labels: Vec<&str> = parsed
            .windows
            .iter()
            .map(|window| window.label.as_str())
            .collect();
        assert_eq!(labels, ["5-hour", "GPT-5 · Weekly", "Code review · Daily"]);
    }

    #[test]
    fn claude_reads_limits_and_model_scoped_probes() {
        let parsed = parse(
            "claude",
            &json!({
                "limits": [
                    {"kind": "session", "percent": 0},
                    {"kind": "weekly_all", "percent": 55, "resets_at": "2026-11-17T04:59:59Z"},
                    {"kind": "weekly_scoped", "percent": 24,
                     "scope": {"model": {"display_name": "Fable"}}}
                ],
                "five_hour": {"utilization": 99},
                "seven_day": {"utilization": 99}
            }),
        )
        .unwrap();
        let labels: Vec<&str> = parsed
            .windows
            .iter()
            .map(|window| window.label.as_str())
            .collect();
        assert_eq!(labels, ["5-hour", "Weekly", "Fable · Weekly"]);
        assert_eq!(parsed.windows[1].used_percent, 55.0);
    }

    #[test]
    fn claude_falls_back_to_the_legacy_weekly_fields() {
        let parsed = parse(
            "claude",
            &json!({
                "five_hour": {"utilization": 1},
                "seven_day": {"utilization": 2},
                "seven_day_opus": {"utilization": 3},
                "limits": []
            }),
        )
        .unwrap();
        assert_eq!(parsed.windows.len(), 3);
        assert_eq!(parsed.windows[2].label, "Weekly · Opus");
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
