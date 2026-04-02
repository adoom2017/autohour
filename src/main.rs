use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{Datelike, Duration, Local, NaiveDate};
use clap::{Parser, Subcommand};
use cookie_store::CookieStore;
use regex::Regex;
use reqwest::Method;
use reqwest::blocking::{Client, ClientBuilder};
use reqwest::cookie::CookieStore as _;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use reqwest_cookie_store::CookieStoreMutex;
use rsa::rand_core::OsRng;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs8::DecodePublicKey};
use scraper::{Html, Selector};
use serde::Serialize;
use serde_json::{Value, json};

const DEFAULT_SERVICE_URL: &str = "https://lh.i.linker.cc/1.UserCenter/Pages/Default.aspx";
const WEEKSYSTEM_APP_URL: &str = "https://weeksystem.linker.cc/wap/index.html?v=1.2.5.5";
const WEEKSYSTEM_CONFIG_URL: &str = "https://weeksystem.linker.cc/statics/config.json";
const MAN_HOUR_API_URL: &str = "https://weeksystem.linker.cc/api/Index/addManHour";
const EDIT_MAN_HOUR_API_URL: &str = "https://weeksystem.linker.cc/api/Index/editMyhour";
const SESSION_PROBE_URL: &str = "https://weeksystem.linker.cc/api/Index/getProjectListByAdmin";
const GET_HOUR_DAILY_URL: &str = "https://weeksystem.linker.cc/api/index/getHourDailyByTime";
const ADD_DAILY_URL: &str = "https://weeksystem.linker.cc/api/Index/addDaily";
const UPDATE_DAILY_URL: &str = "https://weeksystem.linker.cc/api/Index/updateDaily";
const COOKIE_FILE: &str = ".linker_session.cookies.json";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

#[derive(Parser)]
#[command(name = "autohour")]
#[command(about = "Linker 日报/工时自动提交工具", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(long, help = "默认提交当天日志；可指定 YYYY-MM-DD")]
    date: Option<String>,
    #[arg(long, default_value = "", help = "外出/出差工时标记，留空表示普通工时")]
    out_work: String,
}

#[derive(Subcommand)]
enum Commands {
    #[command(visible_alias = "l")]
    Login,
    #[command(visible_alias = "add")]
    AddManHour {
        #[arg(long)]
        year: i32,
        #[arg(long)]
        month: u32,
        #[arg(long)]
        day: u32,
        #[arg(long)]
        workhour: f64,
        #[arg(long)]
        project_id: i64,
        #[arg(long, default_value = "")]
        remark: String,
        #[arg(long, default_value = "")]
        out_work: String,
    },
    #[command(visible_alias = "s")]
    Submit,
}

#[derive(Debug, Clone)]
struct LoginForm {
    action_url: String,
    lt: String,
    execution: String,
    event_id: String,
    pubkey_b64: String,
}

#[derive(Debug, Clone)]
struct ParsedLog {
    target_date: NaiveDate,
    log_path: PathBuf,
    work_record: String,
    tomorrow: String,
    undone: String,
    concert: String,
    daily_remark: String,
    workhour: f64,
}

#[derive(Debug, Clone)]
struct DaySnapshot {
    daily: Option<Value>,
    workhour: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct LoginResult {
    ok: bool,
    cookie_name: String,
    cookie_domain: String,
}

#[derive(Clone)]
struct LinkerClient {
    username: String,
    password: String,
    client: Client,
    cookie_store: Arc<CookieStoreMutex>,
    cookie_file: PathBuf,
}

impl LinkerClient {
    fn new(username: String, password: String, cookie_file: PathBuf) -> Result<Self> {
        let cookie_store = load_cookie_store(&cookie_file)?;
        let client = ClientBuilder::new()
            .cookie_provider(cookie_store.clone())
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            username,
            password,
            client,
            cookie_store,
            cookie_file,
        })
    }

    fn save_cookies(&self) -> Result<()> {
        save_cookie_store(&self.cookie_store, &self.cookie_file)
    }

    fn session_cookie(&self) -> Result<Option<(String, String)>> {
        let weeksystem_url =
            url::Url::parse(WEEKSYSTEM_APP_URL).context("invalid weeksystem application URL")?;
        if let Some(header) = self.cookie_store.cookies(&weeksystem_url) {
            let header_text = header
                .to_str()
                .context("invalid cookie header returned by store")?;
            let mut fallback: Option<(String, String)> = None;
            for item in header_text.split(';') {
                let trimmed = item.trim();
                let Some((name, value)) = trimmed.split_once('=') else {
                    continue;
                };
                if name == "session_for:index_php" || name == "session_for%3Aindex_php" {
                    let current = (name.to_string(), value.to_string());
                    if current.1.starts_with("ST-") {
                        return Ok(Some(current));
                    }
                    fallback = Some(current);
                }
            }
            if fallback.is_some() {
                return Ok(fallback);
            }
        }
        let store = self
            .cookie_store
            .lock()
            .map_err(|_| anyhow!("cookie store lock poisoned"))?;
        let mut fallback: Option<(String, String)> = None;
        for cookie in store.iter_any() {
            if cookie
                .domain()
                .is_some_and(|domain| domain.contains("weeksystem.linker.cc"))
                && (cookie.name() == "session_for:index_php"
                    || cookie.name() == "session_for%3Aindex_php")
            {
                let current = (cookie.name().to_string(), cookie.value().to_string());
                if current.1.starts_with("ST-") {
                    return Ok(Some(current));
                }
                fallback = Some(current);
            }
        }
        Ok(fallback)
    }

    fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<Vec<u8>>,
        extra_headers: Option<HeaderMap>,
    ) -> Result<(String, String)> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        if let Some(extra) = extra_headers {
            for (key, value) in extra {
                if let Some(name) = key {
                    headers.insert(name, value);
                }
            }
        }
        let mut request = self.client.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request
            .send()
            .with_context(|| format!("request failed: {url}"))?;
        let final_url = response.url().to_string();
        let body_text = response.text().context("failed to read response body")?;
        Ok((final_url, body_text))
    }

    fn fetch_login_form(&self, login_url: &str) -> Result<Option<LoginForm>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        let (final_url, body) = self.request(Method::GET, login_url, None, Some(headers))?;
        let document = Html::parse_document(&body);
        let form_selector = Selector::parse("form#fm1").unwrap();
        let input_selector = Selector::parse("input").unwrap();
        let Some(form) = document.select(&form_selector).next() else {
            return Ok(None);
        };
        let action = form
            .value()
            .attr("action")
            .ok_or_else(|| anyhow!("login form action missing"))?;
        let mut lt = String::new();
        let mut execution = None;
        let mut event_id = "submit".to_string();
        let mut pubkey = None;
        for input in document.select(&input_selector) {
            let value = input.value();
            let name = value.attr("name").unwrap_or_default();
            match name {
                "lt" => lt = value.attr("value").unwrap_or_default().to_string(),
                "execution" => {
                    execution = Some(value.attr("value").unwrap_or_default().to_string())
                }
                "_eventId" => {
                    event_id = value.attr("value").unwrap_or("submit").to_string();
                }
                "pubkey" => pubkey = Some(value.attr("value").unwrap_or_default().to_string()),
                _ => {}
            }
        }
        let action_url = url::Url::parse(&final_url)
            .context("invalid final login URL")?
            .join(action)
            .context("failed to resolve form action")?
            .to_string();
        Ok(Some(LoginForm {
            action_url,
            lt,
            execution: execution.ok_or_else(|| anyhow!("execution field missing"))?,
            event_id,
            pubkey_b64: pubkey.ok_or_else(|| anyhow!("pubkey field missing"))?,
        }))
    }

    fn cas_login(&self, login_url: &str) -> Result<(String, String)> {
        let form = self.fetch_login_form(login_url)?;
        if let Some(form) = form {
            let encrypted_password = rsa_encrypt_password(&self.password, &form.pubkey_b64)?;
            let payload = [
                ("username", self.username.as_str()),
                ("password", encrypted_password.as_str()),
                ("lt", form.lt.as_str()),
                ("execution", form.execution.as_str()),
                ("_eventId", form.event_id.as_str()),
                ("submit", "登录"),
            ];
            let encoded =
                serde_urlencoded::to_string(payload).context("failed to encode login form")?;
            let mut headers = HeaderMap::new();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded"),
            );
            headers.insert(
                ACCEPT,
                HeaderValue::from_static(
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                ),
            );
            self.request(
                Method::POST,
                &form.action_url,
                Some(encoded.into_bytes()),
                Some(headers),
            )?;
        }
        let cookie = self.activate_weeksystem_session()?.ok_or_else(|| {
            anyhow!("CAS login completed but weeksystem session was not activated")
        })?;
        self.save_cookies()?;
        Ok(cookie)
    }

    fn relogin(&self) -> Result<(String, String)> {
        self.cas_login(&default_login_url())
    }

    fn ensure_session(&self) -> Result<(String, String)> {
        if let Some(cookie) = self.session_cookie()? {
            return Ok(cookie);
        }
        self.cas_login(&default_login_url())
    }

    fn activate_weeksystem_session(&self) -> Result<Option<(String, String)>> {
        self.visit_weeksystem_landing()?;
        if let Some(cookie) = self.session_cookie()? {
            if cookie.1.starts_with("ST-") {
                return Ok(Some(cookie));
            }
        }
        let _ = self.post_json(SESSION_PROBE_URL, &json!({}));
        if let Some(cookie) = self.session_cookie()? {
            if cookie.1.starts_with("ST-") {
                return Ok(Some(cookie));
            }
        }
        Ok(None)
    }

    fn visit_weeksystem_landing(&self) -> Result<()> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        let _ = self.request(Method::GET, WEEKSYSTEM_APP_URL, None, Some(headers.clone()));
        let _ = self.request(Method::GET, WEEKSYSTEM_CONFIG_URL, None, Some(headers));
        Ok(())
    }

    fn post_json(&self, url: &str, payload: &Value) -> Result<Value> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=utf-8"),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        headers.insert(
            "X-Requested-With",
            HeaderValue::from_static("XMLHttpRequest"),
        );
        let (_, body) = self.request(
            Method::POST,
            url,
            Some(serde_json::to_vec(payload).context("failed to encode JSON payload")?),
            Some(headers),
        )?;
        let parsed: Value = serde_json::from_str(&body)
            .with_context(|| format!("unexpected non-JSON response from {url}"))?;
        if parsed.get("code").and_then(Value::as_i64) == Some(401) {
            bail!("AUTH_REQUIRED");
        }
        if let Some(code) = parsed.get("code") {
            let ok = code.as_i64().map(|v| v == 1 || v == 200).unwrap_or(false)
                || code
                    .as_str()
                    .map(|v| v == "1" || v == "200")
                    .unwrap_or(false);
            if !ok {
                let message = parsed
                    .get("msg")
                    .and_then(Value::as_str)
                    .or_else(|| parsed.get("message").and_then(Value::as_str))
                    .unwrap_or("request failed");
                bail!(message.to_string());
            }
        }
        self.save_cookies()?;
        Ok(parsed)
    }

    fn post_json_with_relogin(&self, url: &str, payload: &Value) -> Result<Value> {
        match self.post_json(url, payload) {
            Ok(value) => Ok(value),
            Err(err) if err.to_string() == "AUTH_REQUIRED" => {
                self.relogin()?;
                self.post_json(url, payload)
            }
            Err(err) => Err(err),
        }
    }

    fn get_day_snapshot(&self, target_date: NaiveDate) -> Result<DaySnapshot> {
        let payload = json!({ "time": resolve_time_token(target_date)? });
        let data = self.post_json_with_relogin(GET_HOUR_DAILY_URL, &payload)?;
        let inner = data.get("data").cloned().unwrap_or_else(|| json!({}));
        let daily = inner.get("daily").cloned().filter(|v| !v.is_null());
        let workhour = inner
            .get("workhour")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(DaySnapshot { daily, workhour })
    }

    fn sync_man_hour(
        &self,
        target_date: NaiveDate,
        project_id: i64,
        workhour: f64,
        remark: &str,
        out_work: &str,
    ) -> Result<Value> {
        let snapshot = self.get_day_snapshot(target_date)?;
        let matches: Vec<&Value> = snapshot
            .workhour
            .iter()
            .filter(|item| item.get("project_id").and_then(Value::as_i64) == Some(project_id))
            .collect();
        if matches.len() > 1 {
            bail!(
                "multiple existing man-hour entries found for project_id={project_id} on {}",
                target_date
            );
        }
        let payload = json!({
            "year": target_date.year(),
            "month": target_date.month(),
            "day": target_date.day(),
            "workhour": workhour,
            "project_id": project_id,
            "remark": remark,
            "outWork": out_work,
        });
        if let Some(existing) = matches.first() {
            let entry_id = existing
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow!("existing man-hour entry missing id"))?;
            let mut object = payload
                .as_object()
                .cloned()
                .ok_or_else(|| anyhow!("payload is not an object"))?;
            object.insert("id".to_string(), json!(entry_id));
            let response =
                self.post_json_with_relogin(EDIT_MAN_HOUR_API_URL, &Value::Object(object))?;
            return Ok(json!({
                "mode": "updated",
                "entry_id": entry_id,
                "response": response
            }));
        }
        let response = self.post_json_with_relogin(MAN_HOUR_API_URL, &payload)?;
        Ok(json!({
            "mode": "created",
            "entry_id": Value::Null,
            "response": response
        }))
    }

    fn sync_daily(&self, parsed_log: &ParsedLog) -> Result<Value> {
        let snapshot = self.get_day_snapshot(parsed_log.target_date)?;
        let daily = snapshot.daily.unwrap_or_else(|| json!({}));
        let payload = json!({
            "id": daily.get("id").cloned().unwrap_or(Value::Null),
            "status": daily.get("status").and_then(Value::as_i64).unwrap_or(0),
            "undone": parsed_log.undone,
            "tomorrow": parsed_log.tomorrow,
            "concert": parsed_log.concert,
            "remark": parsed_log.daily_remark,
            "time": resolve_time_token(parsed_log.target_date)?,
        });
        if payload.get("id").is_some_and(|value| !value.is_null()) {
            let response = self.post_json_with_relogin(UPDATE_DAILY_URL, &payload)?;
            return Ok(json!({
                "mode": "updated",
                "daily_id": payload.get("id").cloned().unwrap_or(Value::Null),
                "response": response
            }));
        }
        let mut object = payload
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("payload is not an object"))?;
        object.remove("id");
        let response = self.post_json_with_relogin(ADD_DAILY_URL, &Value::Object(object))?;
        Ok(json!({
            "mode": "created",
            "daily_id": Value::Null,
            "response": response
        }))
    }

    fn submit_from_log(
        &self,
        parsed_log: &ParsedLog,
        project_id: i64,
        out_work: &str,
    ) -> Result<Value> {
        self.ensure_session()?;
        let man_hour = self.sync_man_hour(
            parsed_log.target_date,
            project_id,
            parsed_log.workhour,
            &parsed_log.work_record,
            out_work,
        )?;
        let daily = self.sync_daily(parsed_log)?;
        Ok(json!({
            "ok": true,
            "date": parsed_log.target_date.to_string(),
            "log_path": parsed_log.log_path,
            "project_id": project_id,
            "workhour": parsed_log.workhour,
            "man_hour": man_hour,
            "daily": daily
        }))
    }
}

fn load_cookie_store(path: &Path) -> Result<Arc<CookieStoreMutex>> {
    let store = if path.exists() {
        load_cookie_store_from_file(path)?
    } else {
        CookieStore::default()
    };
    Ok(Arc::new(CookieStoreMutex::new(store)))
}

fn save_cookie_store(store: &Arc<CookieStoreMutex>, path: &Path) -> Result<()> {
    let file = File::create(path)
        .with_context(|| format!("failed to create cookie file {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let guard = store
        .lock()
        .map_err(|_| anyhow!("cookie store lock poisoned"))?;
    serde_json::to_writer(&mut writer, &*guard)
        .map_err(|err| anyhow!("failed to save cookie store: {err}"))
}

fn load_cookie_store_from_file(path: &Path) -> Result<CookieStore> {
    let file = File::open(path)
        .with_context(|| format!("failed to open cookie file {}", path.display()))?;
    match serde_json::from_reader(BufReader::new(file)) {
        Ok(store) => Ok(store),
        Err(primary_err) => load_cookie_store_legacy(path).map_err(|legacy_err| {
            anyhow!("failed to load cookie store: {primary_err}; legacy fallback also failed: {legacy_err}")
        }),
    }
}

#[allow(deprecated)]
fn load_cookie_store_legacy(path: &Path) -> Result<CookieStore> {
    let file = File::open(path)
        .with_context(|| format!("failed to open cookie file {}", path.display()))?;
    CookieStore::load_json(BufReader::new(file))
        .map_err(|err| anyhow!("failed to load legacy cookie store: {err}"))
}

fn default_login_url() -> String {
    format!(
        "https://login.linker.cc/login/login?service={}",
        urlencoding::encode(DEFAULT_SERVICE_URL)
    )
}

fn rsa_encrypt_password(password: &str, pubkey_b64: &str) -> Result<String> {
    let der = BASE64
        .decode(pubkey_b64)
        .context("failed to decode login public key")?;
    let public_key =
        RsaPublicKey::from_public_key_der(&der).context("failed to parse RSA public key")?;
    let encrypted = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, password.as_bytes())
        .context("failed to encrypt password")?;
    Ok(BASE64.encode(encrypted))
}

fn env_credentials() -> Result<(String, String)> {
    let username = env::var("LINKER_USERNAME").context("missing LINKER_USERNAME")?;
    let password = env::var("LINKER_PASSWORD").context("missing LINKER_PASSWORD")?;
    Ok((username, password))
}

fn env_project_id() -> Result<i64> {
    env::var("LINKER_PROJECT_ID")
        .context("missing LINKER_PROJECT_ID")?
        .parse()
        .context("LINKER_PROJECT_ID must be an integer")
}

fn env_log_dir() -> Result<PathBuf> {
    let dir = PathBuf::from(env::var("AUTOHOUR_LOG_DIR").context("missing AUTOHOUR_LOG_DIR")?);
    if !dir.is_dir() {
        bail!("log directory does not exist: {}", dir.display());
    }
    Ok(dir)
}

fn parse_target_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").context("date must use YYYY-MM-DD format")
}

fn resolve_time_token(target_date: NaiveDate) -> Result<&'static str> {
    let today = Local::now().date_naive();
    if target_date == today {
        return Ok("T");
    }
    if target_date == today - Duration::days(1) {
        return Ok("Y");
    }
    bail!("weeksystem only supports querying/submitting today or yesterday via this CLI")
}

fn parse_markdown_sections(markdown_text: &str) -> HashMap<String, String> {
    let mut sections = HashMap::from([
        ("work_record".to_string(), String::new()),
        ("tomorrow".to_string(), String::new()),
        ("undone".to_string(), String::new()),
        ("concert".to_string(), String::new()),
        ("daily_remark".to_string(), String::new()),
    ]);
    let title_map = HashMap::from([
        ("工作记录", "work_record"),
        ("明日计划", "tomorrow"),
        ("未完成工作", "undone"),
        ("需协调工作", "concert"),
        ("备注", "daily_remark"),
    ]);
    let heading_re = Regex::new(r"^(#{1,6})\s+(.*?)\s*$").unwrap();
    let mut current_key: Option<&str> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in markdown_text.lines() {
        if let Some(caps) = heading_re.captures(line) {
            let level = caps.get(1).map(|m| m.as_str().len()).unwrap_or(0);
            let title = caps.get(2).map(|m| m.as_str().trim()).unwrap_or_default();
            if current_key.is_some() && level <= 2 {
                if let Some(key) = current_key {
                    sections.insert(key.to_string(), current_lines.join("\n").trim().to_string());
                }
                current_key = None;
                current_lines.clear();
            }
            if level == 2 {
                current_key = title_map.get(title).copied();
                if current_key.is_some() {
                    current_lines.clear();
                }
            } else if current_key.is_some() {
                current_lines.push(line.to_string());
            }
            continue;
        }
        if current_key.is_some() {
            current_lines.push(line.to_string());
        }
    }
    if let Some(key) = current_key {
        sections.insert(key.to_string(), current_lines.join("\n").trim().to_string());
    }
    sections
}

fn parse_workhour(text: &str) -> Result<f64> {
    let re = Regex::new(r"(?im)^\s*工时\s*[：:]\s*([0-9]+(?:\.[0-9]+)?)\s*h?\s*$").unwrap();
    let captures = re.captures(text).ok_or_else(|| {
        anyhow!("missing workhour declaration in '## 工作记录'; expected a line like '工时: 8'")
    })?;
    let value: f64 = captures[1].parse().context("invalid workhour value")?;
    if value <= 0.0 {
        bail!("workhour must be greater than 0");
    }
    if ((value * 2.0).round() - (value * 2.0)).abs() > 1e-9 {
        bail!("workhour must use 0.5 hour increments");
    }
    Ok(value)
}

fn validate_tomorrow(text: &str) -> Result<String> {
    let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if normalized.chars().count() < 4 {
        bail!("'## 明日计划' must contain at least 4 non-whitespace characters");
    }
    Ok(text.to_string())
}

fn load_log_file(log_dir: &Path, target_date: NaiveDate) -> Result<ParsedLog> {
    let log_path = log_dir.join(format!("{target_date}.md"));
    if !log_path.is_file() {
        bail!("log file not found: {}", log_path.display());
    }
    let markdown = fs::read_to_string(&log_path)
        .with_context(|| format!("failed to read {}", log_path.display()))?;
    let sections = parse_markdown_sections(&markdown);
    let work_record = sections
        .get("work_record")
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    if work_record.is_empty() {
        bail!("missing content under '## 工作记录'");
    }
    let tomorrow = validate_tomorrow(sections.get("tomorrow").cloned().unwrap_or_default().trim())?;
    Ok(ParsedLog {
        target_date,
        log_path,
        work_record: work_record.clone(),
        tomorrow,
        undone: sections
            .get("undone")
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string(),
        concert: sections
            .get("concert")
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string(),
        daily_remark: sections
            .get("daily_remark")
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string(),
        workhour: parse_workhour(&work_record)?,
    })
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(value).context("failed to serialize JSON")?
    );
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (username, password) = env_credentials()?;
    let client = LinkerClient::new(username, password, PathBuf::from(COOKIE_FILE))?;

    match cli.command {
        Some(Commands::Login) => {
            let (cookie_name, cookie_value) = client.cas_login(&default_login_url())?;
            let result = LoginResult {
                ok: true,
                cookie_name,
                cookie_domain: "weeksystem.linker.cc".to_string(),
            };
            let _ = cookie_value;
            print_json(&result)?;
        }
        Some(Commands::AddManHour {
            year,
            month,
            day,
            workhour,
            project_id,
            remark,
            out_work,
        }) => {
            client.ensure_session()?;
            let payload = json!({
                "year": year,
                "month": month,
                "day": day,
                "workhour": workhour,
                "project_id": project_id,
                "remark": remark,
                "outWork": out_work
            });
            let response = client.post_json_with_relogin(MAN_HOUR_API_URL, &payload)?;
            print_json(&response)?;
        }
        Some(Commands::Submit) | None => {
            let target_date = cli
                .date
                .as_deref()
                .map(parse_target_date)
                .transpose()?
                .unwrap_or_else(|| Local::now().date_naive());
            let parsed_log = load_log_file(&env_log_dir()?, target_date)?;
            let result = client.submit_from_log(&parsed_log, env_project_id()?, &cli.out_work)?;
            print_json(&result)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    const MARKDOWN_TEXT: &str = r#"
# 2026-04-02

## 工作记录
工时: 8
- 完成自动提报脚本联调
- 修复接口会话续期问题

## 明日计划
继续完善日志解析和失败告警

## 未完成工作
日报自动化收尾

## 需协调工作
确认项目配置

## 备注
无
"#;

    #[test]
    fn parses_markdown_sections() {
        let sections = parse_markdown_sections(MARKDOWN_TEXT);
        assert!(sections["work_record"].contains("工时: 8"));
        assert_eq!(sections["tomorrow"].trim(), "继续完善日志解析和失败告警");
    }

    #[test]
    fn parses_workhour() {
        assert_eq!(parse_workhour("工时：7.5h").unwrap(), 7.5);
        assert!(parse_workhour("今天写代码").is_err());
    }

    #[test]
    fn validates_tomorrow() {
        assert!(validate_tomorrow("小欧助手相关工作").is_ok());
        assert!(validate_tomorrow("abc").is_err());
    }

    #[test]
    fn resolves_time_token() {
        let today = Local::now().date_naive();
        assert_eq!(resolve_time_token(today).unwrap(), "T");
        assert_eq!(resolve_time_token(today - Duration::days(1)).unwrap(), "Y");
    }

    #[test]
    fn loads_log_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("2026-04-02.md");
        fs::write(&path, MARKDOWN_TEXT).unwrap();
        let parsed =
            load_log_file(temp.path(), NaiveDate::from_ymd_opt(2026, 4, 2).unwrap()).unwrap();
        assert_eq!(parsed.workhour, 8.0);
        assert!(parsed.work_record.contains("修复接口会话续期问题"));
    }
}
