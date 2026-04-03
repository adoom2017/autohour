use std::collections::HashMap;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::thread;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveTime, TimeZone};
use clap::{Parser, Subcommand};
use cookie_store::CookieStore;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use regex::Regex;
use fs2::FileExt;
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
#[cfg(target_os = "macos")]
use tao::event::{Event, StartCause};
#[cfg(target_os = "macos")]
use mac_notification_sys::{MainButton, Notification, set_application};
#[cfg(target_os = "macos")]
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
#[cfg(target_os = "macos")]
use tray_icon::TrayIconBuilder;
#[cfg(target_os = "macos")]
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};

const DEFAULT_SERVICE_URL: &str = "https://lh.i.linker.cc/1.UserCenter/Pages/Default.aspx";
const WEEKSYSTEM_APP_URL: &str = "https://weeksystem.linker.cc/wap/index.html?v=1.2.5.5";
const WEEKSYSTEM_CONFIG_URL: &str = "https://weeksystem.linker.cc/statics/config.json";
const MAN_HOUR_API_URL: &str = "https://weeksystem.linker.cc/api/Index/addManHour";
const EDIT_MAN_HOUR_API_URL: &str = "https://weeksystem.linker.cc/api/Index/editMyhour";
const SESSION_PROBE_URL: &str = "https://weeksystem.linker.cc/api/Index/getProjectListByAdmin";
const GET_HOUR_DAILY_URL: &str = "https://weeksystem.linker.cc/api/index/getHourDailyByTime";
const ADD_DAILY_URL: &str = "https://weeksystem.linker.cc/api/Index/addDaily";
const UPDATE_DAILY_URL: &str = "https://weeksystem.linker.cc/api/Index/updateDaily";
const DAILY_REPORT_API_URL: &str = "https://weeksystem.linker.cc/api/datareport/daily";
const COOKIE_FILE: &str = ".linker_session.cookies.json";
const HOLIDAY_DIR: &str = "holidays";
const ENV_FILE: &str = ".env";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
#[cfg(target_os = "macos")]
const MENU_ID_SUBMIT_TODAY: &str = "submit_today";
#[cfg(target_os = "macos")]
const MENU_ID_CHECK_MISSING: &str = "check_missing";
#[cfg(target_os = "macos")]
const MENU_ID_REFRESH_LOGIN: &str = "refresh_login";
#[cfg(target_os = "macos")]
const MENU_ID_ENABLE_LOGIN: &str = "enable_login_item";
#[cfg(target_os = "macos")]
const MENU_ID_DISABLE_LOGIN: &str = "disable_login_item";
#[cfg(target_os = "macos")]
const MENU_ID_QUIT: &str = "quit";
#[cfg(target_os = "macos")]
const LAUNCH_AGENT_LABEL: &str = "cc.linker.autohour";

#[derive(Parser)]
#[command(name = "autohour")]
#[command(
    about = "Linker 日报/工时自动提交工具",
    long_about = "默认会从日志目录读取当天日志并提交工时和日报。也支持检查缺报、刷新登录会话、手动提交工时和前台定时执行。"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    #[arg(long, help = "提交指定日期日志，格式 YYYY-MM-DD；默认当天")]
    date: Option<String>,
    #[arg(
        long,
        default_value = "",
        help = "提交日志时附带的 outWork 值；留空表示普通工时"
    )]
    out_work: String,
}

#[derive(Subcommand)]
enum Commands {
    #[command(visible_alias = "l", about = "登录 Linker 并刷新本地会话 cookie")]
    Login,
    #[command(visible_alias = "add", about = "手动提交一条工时记录")]
    AddManHour {
        #[arg(long, help = "年份，例如 2026")]
        year: i32,
        #[arg(long, help = "月份，1 到 12")]
        month: u32,
        #[arg(long, help = "日期，1 到 31")]
        day: u32,
        #[arg(long, help = "工时，必须是 0.5 的整数倍")]
        workhour: f64,
        #[arg(long, help = "项目 ID")]
        project_id: i64,
        #[arg(long, default_value = "", help = "工时备注内容")]
        remark: String,
        #[arg(long, default_value = "", help = "外出/出差工时标记")]
        out_work: String,
    },
    #[command(visible_alias = "s", about = "按日志文件提交工时和日报")]
    Submit,
    #[command(visible_alias = "c", about = "检查指定月份的实际缺报工作日日报")]
    CheckMissing {
        #[arg(long, help = "年份，默认当前年")]
        year: Option<i32>,
        #[arg(long, help = "月份，默认当前月")]
        month: Option<u32>,
    },
    #[command(about = "启动 macOS 菜单栏应用，并启用上午/晚上的自动日志提醒")]
    Tray,
    #[command(about = "安装开机自动启动的 LaunchAgent")]
    InstallLaunchAgent,
    #[command(about = "卸载开机自动启动的 LaunchAgent")]
    UninstallLaunchAgent,
    #[command(visible_alias = "d", about = "以前台常驻方式按时间自动提交当天日志")]
    Daemon {
        #[arg(
            long,
            help = "每天执行时间，格式 HH:MM；默认读取 AUTOHOUR_SCHEDULE_AT，未配置时使用 18:00"
        )]
        at: Option<String>,
    },
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
struct MissingDailyResult {
    ok: bool,
    year: i32,
    month: u32,
    reported_days: Vec<u32>,
    raw_nowrite_days: Vec<u32>,
    excluded_today: Vec<u32>,
    excluded_non_workdays: Vec<u32>,
    missing_workdays: Vec<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct HolidayConfig {
    holidays: Vec<String>,
    #[serde(default)]
    makeup_workdays: Vec<String>,
}

#[derive(Debug, Serialize)]
struct LoginResult {
    ok: bool,
    cookie_name: String,
    cookie_domain: String,
}

#[derive(Debug, Clone)]
struct NotificationConfig {
    telegram: Option<TelegramConfig>,
    email: Option<EmailConfig>,
}

#[derive(Debug, Clone)]
struct TelegramConfig {
    bot_token: String,
    chat_id: String,
}

#[derive(Debug, Clone)]
struct EmailConfig {
    smtp_host: String,
    smtp_port: u16,
    smtp_username: String,
    smtp_password: String,
    from: String,
    to: String,
    starttls: bool,
}

#[cfg(target_os = "macos")]
enum TrayUserEvent {
    SubmitToday,
    CheckCurrentMonth,
    RefreshLogin,
    EnableLaunchAtLogin,
    DisableLaunchAtLogin,
    Quit,
    TaskStarted(&'static str),
    TaskFinished(TaskOutcome),
}

#[cfg(target_os = "macos")]
enum TaskOutcome {
    Success {
        title: String,
        body: String,
        status: String,
        notify_remote: bool,
    },
    Failure {
        title: String,
        body: String,
        status: String,
        notify_remote: bool,
    },
}

#[cfg(target_os = "macos")]
enum NotificationWork {
    Send {
        title: String,
        body: String,
        notify_remote: bool,
    },
}

#[cfg(target_os = "macos")]
struct TrayMenu {
    _tray: tray_icon::TrayIcon,
    submit_item: MenuItem,
    check_item: MenuItem,
    login_item: MenuItem,
    enable_login_item: MenuItem,
    disable_login_item: MenuItem,
    status_item: MenuItem,
    quit_item: MenuItem,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct ReminderState {
    morning: ReminderSlotState,
    evening: ReminderSlotState,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct ReminderSlotState {
    target_date: Option<NaiveDate>,
    last_sent_at: Option<DateTime<Local>>,
    observed_in_window: bool,
    catchup_processed: bool,
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

    fn check_missing_daily(&self, year: i32, month: u32) -> Result<MissingDailyResult> {
        validate_year_month(year, month)?;
        self.ensure_session()?;
        let payload = json!({ "year": year, "month": month });
        let response = self.post_json_with_relogin(DAILY_REPORT_API_URL, &payload)?;
        let data = response.get("data").cloned().unwrap_or_else(|| json!({}));
        let reported_days = parse_reported_days(data.get("write"))?;
        let raw_nowrite_days = parse_day_list(data.get("nowrite"))?;
        let mut excluded_today = Vec::new();
        let mut excluded_non_workdays = Vec::new();
        let mut missing_workdays = Vec::new();
        let today = Local::now().date_naive();

        for day in raw_nowrite_days.iter().copied() {
            let Some(date) = NaiveDate::from_ymd_opt(year, month, day) else {
                continue;
            };
            if date == today {
                excluded_today.push(day);
                continue;
            }
            if !is_linker_report_workday(date)? {
                excluded_non_workdays.push(day);
                continue;
            }
            missing_workdays.push(day);
        }

        Ok(MissingDailyResult {
            ok: true,
            year,
            month,
            reported_days,
            raw_nowrite_days,
            excluded_today,
            excluded_non_workdays,
            missing_workdays,
        })
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

fn load_env_files() -> Result<()> {
    if let Ok(path) = env::var("AUTOHOUR_ENV_FILE") {
        let env_path = PathBuf::from(path);
        if env_path.is_file() {
            load_env_file(&env_path)?;
            return Ok(());
        }
    }
    for candidate in default_env_candidates()? {
        if candidate.is_file() {
            load_env_file(&candidate)?;
            break;
        }
    }
    Ok(())
}

fn load_env_file(path: &Path) -> Result<()> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read env file {}", path.display()))?;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if env::var_os(key).is_some() && !should_override_env_key(key) {
            continue;
        }
        let value = parse_env_value(value.trim());
        unsafe {
            env::set_var(key, value);
        }
    }
    Ok(())
}

fn should_override_env_key(key: &str) -> bool {
    key.starts_with("LINKER_")
        || key.starts_with("AUTOHOUR_")
        || key.starts_with("TELEGRAM_")
        || key.starts_with("SMTP_")
}

fn parse_env_value(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let first = bytes[0];
        let last = bytes[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn default_env_candidates() -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    candidates.push(PathBuf::from(ENV_FILE));
    if let Ok(exe_dir) = executable_dir() {
        candidates.push(exe_dir.join(ENV_FILE));
        if let Some(resources_dir) = bundled_resources_dir() {
            candidates.push(resources_dir.join(ENV_FILE));
        }
    }
    if let Some(app_support_dir) = app_support_dir() {
        candidates.push(app_support_dir.join(ENV_FILE));
    }
    Ok(candidates)
}

fn executable_dir() -> Result<PathBuf> {
    let exe = env::current_exe().context("failed to locate current executable")?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to locate executable directory"))
}

fn bundled_resources_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    let resources_dir = contents_dir.join("Resources");
    resources_dir.is_dir().then_some(resources_dir)
}

fn app_support_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("autohour"),
    )
}

fn ensure_app_support_dir() -> Result<Option<PathBuf>> {
    let Some(dir) = app_support_dir() else {
        return Ok(None);
    };
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create app support dir {}", dir.display()))?;
    Ok(Some(dir))
}

fn default_cookie_file_path() -> Result<PathBuf> {
    if let Some(dir) = ensure_app_support_dir()? {
        return Ok(dir.join(COOKIE_FILE));
    }
    Ok(PathBuf::from(COOKIE_FILE))
}

fn instance_lock_path() -> Result<PathBuf> {
    if let Some(dir) = ensure_app_support_dir()? {
        return Ok(dir.join("autohour.lock"));
    }
    Ok(PathBuf::from("autohour.lock"))
}

fn log_file_path() -> Result<PathBuf> {
    if let Some(dir) = ensure_app_support_dir()? {
        let log_dir = dir.join("logs");
        fs::create_dir_all(&log_dir)
            .with_context(|| format!("failed to create log dir {}", log_dir.display()))?;
        return Ok(log_dir.join("autohour.log"));
    }
    Ok(PathBuf::from("autohour.log"))
}

fn append_log_line(message: &str) {
    let Ok(path) = log_file_path() else {
        return;
    };
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, format!("[{timestamp}] {message}\n").as_bytes()));
}

fn should_enforce_single_instance(cli: &Cli) -> bool {
    bundled_resources_dir().is_some() || matches!(cli.command, Some(Commands::Tray))
}

fn acquire_instance_guard(cli: &Cli) -> Result<Option<File>> {
    if !should_enforce_single_instance(cli) {
        return Ok(None);
    }
    let path = instance_lock_path()?;
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open instance lock {}", path.display()))?;
    if file.try_lock_exclusive().is_err() {
        append_log_line("another Autohour instance is already running; exiting");
        return Ok(None);
    }
    Ok(Some(file))
}

#[cfg(target_os = "macos")]
fn launch_agents_dir() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library").join("LaunchAgents"))
}

#[cfg(target_os = "macos")]
fn launch_agent_plist_path() -> Result<PathBuf> {
    Ok(launch_agents_dir()?.join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn launch_agent_installed() -> bool {
    launch_agent_plist_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn install_launch_agent() -> Result<PathBuf> {
    let plist_path = launch_agent_plist_path()?;
    let parent = plist_path
        .parent()
        .ok_or_else(|| anyhow!("failed to resolve LaunchAgents directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;

    let executable = env::current_exe().context("failed to resolve current executable")?;
    let env_file = default_env_candidates()?
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| app_support_dir().map(|dir| dir.join(ENV_FILE)));
    let holiday_dir = env::var("AUTOHOUR_HOLIDAY_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| bundled_resources_dir().map(|dir| dir.join(HOLIDAY_DIR)))
        .or_else(|| app_support_dir().map(|dir| dir.join(HOLIDAY_DIR)));

    let mut plist = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>cc.linker.autohour</string>
  <key>ProgramArguments</key>
  <array>
"#,
    );
    plist.push_str(&format!(
        "    <string>{}</string>\n    <string>tray</string>\n",
        xml_escape(&executable.display().to_string())
    ));
    plist.push_str(
        r#"  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <false/>
"#,
    );
    if env_file.is_some() || holiday_dir.is_some() {
        plist.push_str("  <key>EnvironmentVariables</key>\n  <dict>\n");
        if let Some(env_file) = env_file {
            plist.push_str(&format!(
                "    <key>AUTOHOUR_ENV_FILE</key>\n    <string>{}</string>\n",
                xml_escape(&env_file.display().to_string())
            ));
        }
        if let Some(holiday_dir) = holiday_dir {
            plist.push_str(&format!(
                "    <key>AUTOHOUR_HOLIDAY_DIR</key>\n    <string>{}</string>\n",
                xml_escape(&holiday_dir.display().to_string())
            ));
        }
        plist.push_str("  </dict>\n");
    }
    plist.push_str("</dict>\n</plist>\n");

    fs::write(&plist_path, plist)
        .with_context(|| format!("failed to write {}", plist_path.display()))?;
    let _ = ProcessCommand::new("launchctl")
        .arg("unload")
        .arg(&plist_path)
        .status();
    let status = ProcessCommand::new("launchctl")
        .arg("load")
        .arg("-w")
        .arg(&plist_path)
        .status()
        .context("failed to execute launchctl load")?;
    if !status.success() {
        bail!("launchctl load failed for {}", plist_path.display());
    }
    Ok(plist_path)
}

#[cfg(target_os = "macos")]
fn uninstall_launch_agent() -> Result<PathBuf> {
    let plist_path = launch_agent_plist_path()?;
    if plist_path.is_file() {
        let _ = ProcessCommand::new("launchctl")
            .arg("unload")
            .arg("-w")
            .arg(&plist_path)
            .status();
        fs::remove_file(&plist_path)
            .with_context(|| format!("failed to remove {}", plist_path.display()))?;
    }
    Ok(plist_path)
}

#[cfg(target_os = "macos")]
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn holiday_config_path(year: i32) -> Result<PathBuf> {
    if let Ok(custom_dir) = env::var("AUTOHOUR_HOLIDAY_DIR") {
        let candidate = PathBuf::from(custom_dir).join(format!("{year}.json"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    let cwd_candidate = PathBuf::from(HOLIDAY_DIR).join(format!("{year}.json"));
    if cwd_candidate.is_file() {
        return Ok(cwd_candidate);
    }

    if let Some(resources_dir) = bundled_resources_dir() {
        let candidate = resources_dir.join(HOLIDAY_DIR).join(format!("{year}.json"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Ok(exe_dir) = executable_dir() {
        let candidate = exe_dir.join(HOLIDAY_DIR).join(format!("{year}.json"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Some(app_support) = app_support_dir() {
        let candidate = app_support.join(HOLIDAY_DIR).join(format!("{year}.json"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    bail!("holiday calendar for {year} is missing; expected holidays/{year}.json in the working directory, app resources, or AUTOHOUR_HOLIDAY_DIR")
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

fn parse_schedule_time(value: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").context("schedule time must use HH:MM format")
}

fn validate_year_month(year: i32, month: u32) -> Result<()> {
    if !(2000..=2100).contains(&year) {
        bail!("year must be in 2000..=2100");
    }
    if !(1..=12).contains(&month) {
        bail!("month must be in 1..=12");
    }
    Ok(())
}

fn default_schedule_time() -> Result<NaiveTime> {
    match env::var("AUTOHOUR_SCHEDULE_AT") {
        Ok(value) => parse_schedule_time(&value),
        Err(_) => parse_schedule_time("18:00"),
    }
}

fn env_notification_config() -> Result<NotificationConfig> {
    Ok(NotificationConfig {
        telegram: telegram_config_from_env(),
        email: email_config_from_env()?,
    })
}

fn telegram_config_from_env() -> Option<TelegramConfig> {
    let bot_token = env::var("TELEGRAM_BOT_TOKEN").ok()?;
    let chat_id = env::var("TELEGRAM_CHAT_ID").ok()?;
    Some(TelegramConfig { bot_token, chat_id })
}

fn email_config_from_env() -> Result<Option<EmailConfig>> {
    let host = env::var("SMTP_HOST").ok();
    let username = env::var("SMTP_USERNAME").ok();
    let password = env::var("SMTP_PASSWORD").ok();
    let from = env::var("SMTP_FROM").ok();
    let to = env::var("SMTP_TO").ok();
    let (Some(smtp_host), Some(smtp_username), Some(smtp_password), Some(from), Some(to)) =
        (host, username, password, from, to)
    else {
        return Ok(None);
    };
    let smtp_port = env::var("SMTP_PORT")
        .ok()
        .map(|value| value.parse::<u16>())
        .transpose()
        .context("SMTP_PORT must be a valid integer")?
        .unwrap_or(587);
    let starttls = env::var("SMTP_STARTTLS")
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(true);
    Ok(Some(EmailConfig {
        smtp_host,
        smtp_port,
        smtp_username,
        smtp_password,
        from,
        to,
        starttls,
    }))
}

fn next_run_after(now: DateTime<Local>, schedule_time: NaiveTime) -> Result<DateTime<Local>> {
    let today = now.date_naive();
    for day_offset in [0_i64, 1_i64] {
        let candidate_date = today + Duration::days(day_offset);
        match Local.from_local_datetime(&candidate_date.and_time(schedule_time)) {
            LocalResult::Single(candidate) if candidate > now => return Ok(candidate),
            LocalResult::Ambiguous(candidate, _) if candidate > now => return Ok(candidate),
            _ => {}
        }
    }
    bail!("failed to compute next scheduled run")
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

fn parse_reported_days(value: Option<&Value>) -> Result<Vec<u32>> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut days = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            continue;
        };
        let day_text = text.split('(').next().unwrap_or(text).trim();
        if day_text.is_empty() {
            continue;
        }
        let day: u32 = day_text
            .parse()
            .with_context(|| format!("invalid reported day value: {text}"))?;
        days.push(day);
    }
    days.sort_unstable();
    days.dedup();
    Ok(days)
}

fn parse_day_list(value: Option<&Value>) -> Result<Vec<u32>> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut days = Vec::new();
    for item in items {
        let day = match item {
            Value::Number(number) => number
                .as_u64()
                .ok_or_else(|| anyhow!("invalid numeric day value: {number}"))?
                as u32,
            Value::String(text) => text
                .parse()
                .with_context(|| format!("invalid day value: {text}"))?,
            _ => continue,
        };
        days.push(day);
    }
    days.sort_unstable();
    days.dedup();
    Ok(days)
}

fn is_linker_report_workday(date: NaiveDate) -> Result<bool> {
    let weekday = date.weekday();
    let is_weekend = matches!(weekday, chrono::Weekday::Sat | chrono::Weekday::Sun);
    let holiday_calendar = china_holiday_calendar(date.year())?;
    if holiday_calendar.makeup_workdays.contains(&date) {
        return Ok(true);
    }
    if holiday_calendar.holidays.contains(&date) {
        return Ok(false);
    }
    Ok(!is_weekend)
}

struct HolidayCalendar {
    holidays: Vec<NaiveDate>,
    makeup_workdays: Vec<NaiveDate>,
}

fn china_holiday_calendar(year: i32) -> Result<HolidayCalendar> {
    let path = holiday_config_path(year)?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read holiday calendar {}", path.display()))?;
    let config: HolidayConfig = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse holiday calendar {}", path.display()))?;
    Ok(HolidayCalendar {
        holidays: parse_holiday_dates(&config.holidays)?,
        makeup_workdays: parse_holiday_dates(&config.makeup_workdays)?,
    })
}

fn parse_holiday_dates(items: &[String]) -> Result<Vec<NaiveDate>> {
    let mut dates = Vec::new();
    for item in items {
        if let Some((start_text, end_text)) = item.split_once("..") {
            let start = parse_holiday_date(start_text.trim())?;
            let end = parse_holiday_date(end_text.trim())?;
            let mut current = start;
            while current <= end {
                dates.push(current);
                current += Duration::days(1);
            }
        } else {
            dates.push(parse_holiday_date(item.trim())?);
        }
    }
    dates.sort_unstable();
    dates.dedup();
    Ok(dates)
}

fn parse_holiday_date(text: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .with_context(|| format!("invalid holiday date: {text}"))
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

fn execute_submit(client: &LinkerClient, target_date: NaiveDate, out_work: &str) -> Result<Value> {
    let parsed_log = load_log_file(&env_log_dir()?, target_date)?;
    client.submit_from_log(&parsed_log, env_project_id()?, out_work)
}

fn execute_check_missing(
    client: &LinkerClient,
    year: Option<i32>,
    month: Option<u32>,
) -> Result<MissingDailyResult> {
    let now = Local::now().date_naive();
    client.check_missing_daily(year.unwrap_or(now.year()), month.unwrap_or(now.month()))
}

fn send_notifications(
    client: &Client,
    config: &NotificationConfig,
    title: &str,
    body: &str,
) -> Result<()> {
    let mut failures = Vec::new();
    if let Some(telegram) = &config.telegram {
        if let Err(err) = send_telegram_notification(client, telegram, title, body) {
            failures.push(format!("telegram: {err}"));
        }
    }
    if let Some(email) = &config.email {
        if let Err(err) = send_email_notification(email, title, body) {
            failures.push(format!("email: {err}"));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    bail!(failures.join("; "))
}

#[cfg(target_os = "macos")]
fn send_macos_notification(title: &str, body: &str) -> Result<()> {
    let _ = set_application("cc.linker.autohour");
    let mut notification = Notification::new();
    notification
        .title(title)
        .message(body)
        .main_button(MainButton::SingleAction("显示"));
    notification
        .send()
        .map_err(|err| anyhow!("failed to send macOS notification: {err}"))?;
    append_log_line(&format!("notification sent via native macOS API: {title}"));
    Ok(())
}

fn send_telegram_notification(
    client: &Client,
    config: &TelegramConfig,
    title: &str,
    body: &str,
) -> Result<()> {
    let response = client
        .post(format!(
            "https://api.telegram.org/bot{}/sendMessage",
            config.bot_token
        ))
        .json(&json!({
            "chat_id": config.chat_id,
            "text": format!("*{}*\n\n{}", escape_markdown_v2(title), escape_markdown_v2(body)),
            "parse_mode": "MarkdownV2"
        }))
        .send()
        .context("failed to send telegram request")?;
    let value: Value = response.json().context("failed to parse telegram response")?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    bail!("telegram API returned failure: {value}");
}

fn send_email_notification(config: &EmailConfig, title: &str, body: &str) -> Result<()> {
    let email = Message::builder()
        .from(config.from.parse::<Mailbox>().context("invalid SMTP_FROM address")?)
        .to(config.to.parse::<Mailbox>().context("invalid SMTP_TO address")?)
        .subject(title)
        .body(body.to_string())
        .context("failed to build email message")?;
    let credentials = Credentials::new(
        config.smtp_username.clone(),
        config.smtp_password.clone(),
    );
    let builder = if config.starttls {
        SmtpTransport::relay(&config.smtp_host).context("failed to create SMTP relay")?
    } else {
        SmtpTransport::builder_dangerous(&config.smtp_host)
    };
    builder
        .port(config.smtp_port)
        .credentials(credentials)
        .build()
        .send(&email)
        .context("failed to send email")?;
    Ok(())
}

fn escape_markdown_v2(text: &str) -> String {
    const SPECIAL: [char; 18] = [
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
    ];
    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        if SPECIAL.contains(&ch) {
            output.push('\\');
        }
        output.push(ch);
    }
    output
}

fn summarize_result(result: &Value) -> String {
    let date = result.get("date").and_then(Value::as_str).unwrap_or("-");
    let project_id = result
        .get("project_id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let workhour = result
        .get("workhour")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let man_hour_mode = result
        .get("man_hour")
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let daily_mode = result
        .get("daily")
        .and_then(|value| value.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    format!(
        "日期: {date}\n项目: {project_id}\n工时: {workhour}\n工时结果: {man_hour_mode}\n日报结果: {daily_mode}"
    )
}

fn summarize_missing_daily(result: &MissingDailyResult) -> String {
    format!(
        "月份: {:04}-{:02}\n已填写: {:?}\n原始未填写: {:?}\n排除当天: {:?}\n排除非工作日: {:?}\n实际缺报工作日: {:?}",
        result.year,
        result.month,
        result.reported_days,
        result.raw_nowrite_days,
        result.excluded_today,
        result.excluded_non_workdays,
        result.missing_workdays
    )
}

fn format_missing_notification_body(result: &MissingDailyResult) -> String {
    if result.missing_workdays.is_empty() {
        return format!("{:02}月暂无缺报", result.month);
    }
    let dates = result
        .missing_workdays
        .iter()
        .map(|day| format!("{}月{}号", result.month, day))
        .collect::<Vec<_>>()
        .join("、");
    format!("检测到缺报：{dates}")
}

#[cfg(target_os = "macos")]
fn dispatch_notification_work(
    client: Client,
    notifications: NotificationConfig,
    work: NotificationWork,
) {
    thread::spawn(move || match work {
        NotificationWork::Send {
            title,
            body,
            notify_remote,
        } => {
            if let Err(err) = send_macos_notification(&title, &body) {
                append_log_line(&format!("failed to send macOS notification: {err:#}"));
            }
            if notify_remote {
                if let Err(err) = send_notifications(&client, &notifications, &title, &body) {
                    append_log_line(&format!("failed to send remote notification: {err:#}"));
                }
            }
        }
    });
}

fn has_daily_report(snapshot: &DaySnapshot) -> bool {
    snapshot
        .daily
        .as_ref()
        .and_then(|daily| daily.get("id"))
        .is_some_and(|id| !id.is_null())
}

#[cfg(target_os = "macos")]
fn is_between_hours(time: NaiveTime, start_hour: u32, end_hour: u32) -> bool {
    let Some(start) = NaiveTime::from_hms_opt(start_hour, 0, 0) else {
        return false;
    };
    let Some(end) = NaiveTime::from_hms_opt(end_hour, 0, 0) else {
        return false;
    };
    time >= start && time < end
}

#[cfg(target_os = "macos")]
fn reset_reminder_slot_for_date(slot: &mut ReminderSlotState, target_date: NaiveDate) {
    if slot.target_date != Some(target_date) {
        slot.target_date = Some(target_date);
        slot.last_sent_at = None;
        slot.observed_in_window = false;
        slot.catchup_processed = false;
    }
}

#[cfg(target_os = "macos")]
fn reminder_due(slot: &ReminderSlotState, now: DateTime<Local>) -> bool {
    slot.last_sent_at
        .map(|sent_at| now - sent_at >= Duration::minutes(30))
        .unwrap_or(true)
}

#[cfg(target_os = "macos")]
fn mark_reminder_sent(slot: &mut ReminderSlotState, now: DateTime<Local>) {
    slot.last_sent_at = Some(now);
}

#[cfg(target_os = "macos")]
fn start_tray_reminder_loop(client: LinkerClient) {
    thread::spawn(move || {
        let mut state = ReminderState::default();
        loop {
            if let Err(err) = process_tray_reminders(&client, &mut state) {
                eprintln!("reminder check failed: {err}");
            }
            thread::sleep(StdDuration::from_secs(60));
        }
    });
}

#[cfg(target_os = "macos")]
fn process_tray_reminders(client: &LinkerClient, state: &mut ReminderState) -> Result<()> {
    let now = Local::now();
    let today = now.date_naive();

    process_time_window_reminder(
        client,
        &mut state.morning,
        now,
        today - Duration::days(1),
        "昨天",
        8,
        10,
    )?;
    process_time_window_reminder(client, &mut state.evening, now, today, "今天", 18, 20)?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn process_time_window_reminder(
    client: &LinkerClient,
    slot: &mut ReminderSlotState,
    now: DateTime<Local>,
    target_date: NaiveDate,
    label: &str,
    start_hour: u32,
    end_hour: u32,
) -> Result<()> {
    reset_reminder_slot_for_date(slot, target_date);
    let in_window = is_between_hours(now.time(), start_hour, end_hour);
    if in_window {
        slot.observed_in_window = true;
        if reminder_due(slot, now) {
            send_missing_daily_reminder(client, slot, now, target_date, label)?;
        }
        return Ok(());
    }

    let Some(end_time) = NaiveTime::from_hms_opt(end_hour, 0, 0) else {
        return Ok(());
    };
    if now.time() >= end_time && !slot.observed_in_window && !slot.catchup_processed {
        slot.catchup_processed = true;
        send_missing_daily_reminder(client, slot, now, target_date, label)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn send_missing_daily_reminder(
    client: &LinkerClient,
    slot: &mut ReminderSlotState,
    now: DateTime<Local>,
    target_date: NaiveDate,
    label: &str,
) -> Result<()> {
    let snapshot = client.get_day_snapshot(target_date)?;
    if has_daily_report(&snapshot) {
        return Ok(());
    }
    let body = format!(
        "{}{}月{}号还没有填报日志",
        label,
        target_date.month(),
        target_date.day()
    );
    send_macos_notification("autohour 日报提醒", &body)?;
    mark_reminder_sent(slot, now);
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_tray_app(
    client: LinkerClient,
    notifications: NotificationConfig,
    default_out_work: String,
) -> Result<()> {
    let event_loop = EventLoopBuilder::<TrayUserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    MenuEvent::set_event_handler(Some({
        let proxy = proxy.clone();
        move |event: MenuEvent| {
            let _ = proxy.send_event(map_menu_event(event));
        }
    }));

    let mut tray_menu: Option<TrayMenu> = None;
    let mut busy = false;

    event_loop.run(move |event, event_loop, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                event_loop.set_activation_policy_at_runtime(ActivationPolicy::Accessory);
                match build_tray_menu() {
                    Ok(menu) => {
                        set_tray_busy_state(&menu, false, "状态：监控中");
                        tray_menu = Some(menu);
                        start_tray_reminder_loop(client.clone());
                    }
                    Err(err) => {
                        eprintln!("failed to initialize tray: {err}");
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::UserEvent(user_event) => match user_event {
                TrayUserEvent::SubmitToday => {
                    if !busy {
                        busy = true;
                        launch_submit_today_task(
                            proxy.clone(),
                            client.clone(),
                            notifications.clone(),
                            default_out_work.clone(),
                        );
                    }
                }
                TrayUserEvent::CheckCurrentMonth => {
                    if !busy {
                        busy = true;
                        launch_check_missing_task(
                            proxy.clone(),
                            client.clone(),
                            notifications.clone(),
                        );
                    }
                }
                TrayUserEvent::RefreshLogin => {
                    if !busy {
                        busy = true;
                        launch_refresh_login_task(proxy.clone(), client.clone(), notifications.clone());
                    }
                }
                TrayUserEvent::EnableLaunchAtLogin => {
                    if !busy {
                        busy = true;
                        launch_install_launch_agent_task(proxy.clone());
                    }
                }
                TrayUserEvent::DisableLaunchAtLogin => {
                    if !busy {
                        busy = true;
                        launch_uninstall_launch_agent_task(proxy.clone());
                    }
                }
                TrayUserEvent::Quit => {
                    *control_flow = ControlFlow::Exit;
                }
                TrayUserEvent::TaskStarted(status) => {
                    if let Some(menu) = tray_menu.as_ref() {
                        set_tray_busy_state(menu, true, status);
                    }
                }
                TrayUserEvent::TaskFinished(outcome) => {
                    busy = false;
                    if let Some(menu) = tray_menu.as_ref() {
                        let status = match &outcome {
                            TaskOutcome::Success { status, .. } => status.as_str(),
                            TaskOutcome::Failure { status, .. } => status.as_str(),
                        };
                        set_tray_busy_state(menu, false, status);
                    }
                    match outcome {
                        TaskOutcome::Success {
                            title,
                            body,
                            notify_remote,
                            ..
                        } => {
                            dispatch_notification_work(
                                client.client.clone(),
                                notifications.clone(),
                                NotificationWork::Send {
                                    title,
                                    body,
                                    notify_remote,
                                },
                            );
                        }
                        TaskOutcome::Failure {
                            title,
                            body,
                            notify_remote,
                            ..
                        } => {
                            dispatch_notification_work(
                                client.client.clone(),
                                notifications.clone(),
                                NotificationWork::Send {
                                    title,
                                    body,
                                    notify_remote,
                                },
                            );
                        }
                    }
                }
            },
            _ => {}
        }
    });
}

#[cfg(target_os = "macos")]
fn build_tray_menu() -> Result<TrayMenu> {
    let status_item = MenuItem::new("状态：监控中", false, None);
    let submit_item = MenuItem::with_id(MENU_ID_SUBMIT_TODAY, "提交今天日志", true, None);
    let check_item = MenuItem::with_id(MENU_ID_CHECK_MISSING, "检查本月缺报", true, None);
    let login_item = MenuItem::with_id(MENU_ID_REFRESH_LOGIN, "刷新登录会话", true, None);
    let enable_login_item =
        MenuItem::with_id(MENU_ID_ENABLE_LOGIN, "开启开机自动启动", true, None);
    let disable_login_item =
        MenuItem::with_id(MENU_ID_DISABLE_LOGIN, "关闭开机自动启动", true, None);
    let quit_item = MenuItem::with_id(MENU_ID_QUIT, "退出", true, None);
    let separator_top = PredefinedMenuItem::separator();
    let separator_middle = PredefinedMenuItem::separator();
    let separator_bottom = PredefinedMenuItem::separator();
    let menu = Menu::new();
    menu.append(&status_item)?;
    menu.append(&separator_top)?;
    menu.append(&submit_item)?;
    menu.append(&check_item)?;
    menu.append(&login_item)?;
    menu.append(&separator_middle)?;
    menu.append(&enable_login_item)?;
    menu.append(&disable_login_item)?;
    menu.append(&separator_bottom)?;
    menu.append(&quit_item)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("autohour")
        .with_icon(build_tray_icon()?)
        .with_icon_as_template(true)
        .build()
        .context("failed to build tray icon")?;

    Ok(TrayMenu {
        _tray: tray,
        submit_item,
        check_item,
        login_item,
        enable_login_item,
        disable_login_item,
        status_item,
        quit_item,
    })
}

#[cfg(target_os = "macos")]
fn map_menu_event(event: MenuEvent) -> TrayUserEvent {
    let id = event.id.0.as_ref();
    match id {
        MENU_ID_SUBMIT_TODAY => TrayUserEvent::SubmitToday,
        MENU_ID_CHECK_MISSING => TrayUserEvent::CheckCurrentMonth,
        MENU_ID_REFRESH_LOGIN => TrayUserEvent::RefreshLogin,
        MENU_ID_ENABLE_LOGIN => TrayUserEvent::EnableLaunchAtLogin,
        MENU_ID_DISABLE_LOGIN => TrayUserEvent::DisableLaunchAtLogin,
        MENU_ID_QUIT => TrayUserEvent::Quit,
        _ => TrayUserEvent::TaskStarted("状态：就绪"),
    }
}

#[cfg(target_os = "macos")]
fn set_tray_busy_state(menu: &TrayMenu, busy: bool, status: &str) {
    menu.submit_item.set_enabled(!busy);
    menu.check_item.set_enabled(!busy);
    menu.login_item.set_enabled(!busy);
    let installed = launch_agent_installed();
    menu.enable_login_item.set_enabled(!busy && !installed);
    menu.disable_login_item.set_enabled(!busy && installed);
    menu.quit_item.set_enabled(true);
    menu.status_item.set_text(status);
}

#[cfg(target_os = "macos")]
fn build_tray_icon() -> Result<tray_icon::Icon> {
    let width = 18;
    let height = 18;
    let mut rgba = vec![0_u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let draw = (x >= 3 && x <= 4 && y >= 5 && y <= 13)
                || (x >= 8 && x <= 9 && y >= 3 && y <= 13)
                || (x >= 13 && x <= 14 && y >= 7 && y <= 13);
            if draw {
                let idx = (y * width + x) * 4;
                rgba[idx] = 0;
                rgba[idx + 1] = 0;
                rgba[idx + 2] = 0;
                rgba[idx + 3] = 255;
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, width as u32, height as u32)
        .context("failed to create tray icon")
}

#[cfg(target_os = "macos")]
fn launch_submit_today_task(
    proxy: EventLoopProxy<TrayUserEvent>,
    client: LinkerClient,
    notifications: NotificationConfig,
    out_work: String,
) {
    thread::spawn(move || {
        let _ = proxy.send_event(TrayUserEvent::TaskStarted("状态：正在提交今天日志"));
        let target_date = Local::now().date_naive();
        let outcome = match execute_submit(&client, target_date, &out_work) {
            Ok(result) => TaskOutcome::Success {
                title: "autohour 提交成功".to_string(),
                body: summarize_result(&result),
                status: "状态：最近一次提交成功".to_string(),
                notify_remote: true,
            },
            Err(err) => TaskOutcome::Failure {
                title: "autohour 提交失败".to_string(),
                body: format!("日期: {target_date}\n错误: {err}"),
                status: "状态：最近一次提交失败".to_string(),
                notify_remote: true,
            },
        };
        let _ = notifications;
        let _ = proxy.send_event(TrayUserEvent::TaskFinished(outcome));
    });
}

#[cfg(target_os = "macos")]
fn launch_check_missing_task(
    proxy: EventLoopProxy<TrayUserEvent>,
    client: LinkerClient,
    notifications: NotificationConfig,
) {
    thread::spawn(move || {
        let _ = proxy.send_event(TrayUserEvent::TaskStarted("状态：正在检查缺报"));
        let outcome = match execute_check_missing(&client, None, None) {
            Ok(result) => {
                let has_missing = !result.missing_workdays.is_empty();
                let title = if has_missing {
                    "autohour 检测到缺报"
                } else {
                    "autohour 缺报检查完成"
                };
                TaskOutcome::Success {
                    title: title.to_string(),
                    body: format_missing_notification_body(&result),
                    status: if has_missing {
                        "状态：检测到缺报".to_string()
                    } else {
                        "状态：本月暂无缺报".to_string()
                    },
                    notify_remote: has_missing,
                }
            }
            Err(err) => TaskOutcome::Failure {
                title: "autohour 缺报检查失败".to_string(),
                body: err.to_string(),
                status: "状态：缺报检查失败".to_string(),
                notify_remote: true,
            },
        };
        let _ = notifications;
        let _ = proxy.send_event(TrayUserEvent::TaskFinished(outcome));
    });
}

#[cfg(target_os = "macos")]
fn launch_refresh_login_task(
    proxy: EventLoopProxy<TrayUserEvent>,
    client: LinkerClient,
    notifications: NotificationConfig,
) {
    thread::spawn(move || {
        let _ = proxy.send_event(TrayUserEvent::TaskStarted("状态：正在刷新登录会话"));
        let outcome = match client.relogin() {
            Ok((cookie_name, _cookie_value)) => TaskOutcome::Success {
                title: "autohour 登录成功".to_string(),
                body: format!("已刷新会话 cookie: {cookie_name}"),
                status: "状态：登录会话已刷新".to_string(),
                notify_remote: false,
            },
            Err(err) => TaskOutcome::Failure {
                title: "autohour 登录失败".to_string(),
                body: err.to_string(),
                status: "状态：登录刷新失败".to_string(),
                notify_remote: true,
            },
        };
        let _ = notifications;
        let _ = proxy.send_event(TrayUserEvent::TaskFinished(outcome));
    });
}

#[cfg(target_os = "macos")]
fn launch_install_launch_agent_task(proxy: EventLoopProxy<TrayUserEvent>) {
    thread::spawn(move || {
        let _ = proxy.send_event(TrayUserEvent::TaskStarted("状态：正在开启开机启动"));
        let outcome = match install_launch_agent() {
            Ok(path) => TaskOutcome::Success {
                title: "autohour 已开启开机启动".to_string(),
                body: format!("已安装启动项：{}", path.display()),
                status: "状态：已开启开机启动".to_string(),
                notify_remote: false,
            },
            Err(err) => TaskOutcome::Failure {
                title: "autohour 开机启动设置失败".to_string(),
                body: err.to_string(),
                status: "状态：开机启动设置失败".to_string(),
                notify_remote: false,
            },
        };
        let _ = proxy.send_event(TrayUserEvent::TaskFinished(outcome));
    });
}

#[cfg(target_os = "macos")]
fn launch_uninstall_launch_agent_task(proxy: EventLoopProxy<TrayUserEvent>) {
    thread::spawn(move || {
        let _ = proxy.send_event(TrayUserEvent::TaskStarted("状态：正在关闭开机启动"));
        let outcome = match uninstall_launch_agent() {
            Ok(path) => TaskOutcome::Success {
                title: "autohour 已关闭开机启动".to_string(),
                body: format!("已移除启动项：{}", path.display()),
                status: "状态：已关闭开机启动".to_string(),
                notify_remote: false,
            },
            Err(err) => TaskOutcome::Failure {
                title: "autohour 关闭开机启动失败".to_string(),
                body: err.to_string(),
                status: "状态：关闭开机启动失败".to_string(),
                notify_remote: false,
            },
        };
        let _ = proxy.send_event(TrayUserEvent::TaskFinished(outcome));
    });
}

#[cfg(not(target_os = "macos"))]
fn run_tray_app(
    _client: LinkerClient,
    _notifications: NotificationConfig,
    _default_out_work: String,
) -> Result<()> {
    bail!("tray mode is only supported on macOS")
}

fn run_daemon(
    client: &LinkerClient,
    schedule_time: NaiveTime,
    out_work: &str,
    notifications: &NotificationConfig,
) -> Result<()> {
    loop {
        let now = Local::now();
        let next_run = next_run_after(now, schedule_time)?;
        let sleep_duration = (next_run - now)
            .to_std()
            .context("failed to compute sleep duration")?;
        eprintln!(
            "next run scheduled at {}",
            next_run.format("%Y-%m-%d %H:%M:%S")
        );
        thread::sleep(StdDuration::from_secs(sleep_duration.as_secs()));

        let target_date = Local::now().date_naive();
        match execute_submit(client, target_date, out_work) {
            Ok(result) => {
                print_json(&result)?;
                let _ = send_notifications(
                    &client.client,
                    notifications,
                    "autohour 提交成功",
                    &summarize_result(&result),
                );
            }
            Err(err) => {
                eprintln!("submit failed: {err}");
                let _ = send_notifications(
                    &client.client,
                    notifications,
                    "autohour 提交失败",
                    &format!("日期: {target_date}\n错误: {err}"),
                );
            }
        }
    }
}

fn cli_from_runtime() -> Cli {
    let mut args: Vec<_> = env::args_os().collect();
    if args.len() == 1 && bundled_resources_dir().is_some() {
        args.push("tray".into());
    }
    Cli::parse_from(args)
}

fn real_main() -> Result<()> {
    load_env_files()?;
    let cli = cli_from_runtime();
    let _instance_guard = acquire_instance_guard(&cli)?;
    if should_enforce_single_instance(&cli) && _instance_guard.is_none() {
        return Ok(());
    }
    let (username, password) = env_credentials()?;
    let client = LinkerClient::new(username, password, default_cookie_file_path()?)?;
    let notifications = env_notification_config()?;

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
            let result = execute_submit(&client, target_date, &cli.out_work)?;
            print_json(&result)?;
            let _ = send_notifications(
                &client.client,
                &notifications,
                "autohour 提交成功",
                &summarize_result(&result),
            );
        }
        Some(Commands::CheckMissing { year, month }) => {
            let result = execute_check_missing(&client, year, month)?;
            print_json(&result)?;
            if !result.missing_workdays.is_empty() {
                let _ = send_notifications(
                    &client.client,
                    &notifications,
                    "autohour 检测到缺报",
                    &summarize_missing_daily(&result),
                );
            }
        }
        Some(Commands::Tray) => {
            run_tray_app(client, notifications, cli.out_work)?;
        }
        Some(Commands::InstallLaunchAgent) => {
            #[cfg(target_os = "macos")]
            {
                let path = install_launch_agent()?;
                print_json(&json!({
                    "ok": true,
                    "launch_agent": path,
                    "enabled": true
                }))?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                bail!("install-launch-agent is only supported on macOS");
            }
        }
        Some(Commands::UninstallLaunchAgent) => {
            #[cfg(target_os = "macos")]
            {
                let path = uninstall_launch_agent()?;
                print_json(&json!({
                    "ok": true,
                    "launch_agent": path,
                    "enabled": false
                }))?;
            }
            #[cfg(not(target_os = "macos"))]
            {
                bail!("uninstall-launch-agent is only supported on macOS");
            }
        }
        Some(Commands::Daemon { at }) => {
            let schedule_time = at
                .as_deref()
                .map(parse_schedule_time)
                .transpose()?
                .unwrap_or(default_schedule_time()?);
            run_daemon(&client, schedule_time, &cli.out_work, &notifications)?;
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    append_log_line("autohour starting");
    match real_main() {
        Ok(()) => {
            append_log_line("autohour exited normally");
            Ok(())
        }
        Err(err) => {
            append_log_line(&format!("autohour error: {err:#}"));
            Err(err)
        }
    }
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
    fn parses_schedule_time() {
        assert_eq!(
            parse_schedule_time("18:30").unwrap(),
            NaiveTime::from_hms_opt(18, 30, 0).unwrap()
        );
        assert!(parse_schedule_time("25:00").is_err());
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

    #[test]
    fn parses_reported_days_with_annotations() {
        let value = json!(["1(迟交)", "2", "12(补写)"]);
        assert_eq!(parse_reported_days(Some(&value)).unwrap(), vec![1, 2, 12]);
    }

    #[test]
    fn identifies_2026_holiday_and_makeup_workday() {
        assert!(!is_linker_report_workday(NaiveDate::from_ymd_opt(2026, 4, 6).unwrap()).unwrap());
        assert!(is_linker_report_workday(NaiveDate::from_ymd_opt(2026, 5, 9).unwrap()).unwrap());
        assert!(!is_linker_report_workday(NaiveDate::from_ymd_opt(2026, 4, 5).unwrap()).unwrap());
        assert!(is_linker_report_workday(NaiveDate::from_ymd_opt(2026, 4, 7).unwrap()).unwrap());
    }

    #[test]
    fn parses_holiday_dates_from_config_format() {
        let parsed = parse_holiday_dates(&[
            "2026-04-04..2026-04-06".to_string(),
            "2026-05-09".to_string(),
        ])
        .unwrap();
        assert_eq!(parsed.len(), 4);
        assert!(parsed.contains(&NaiveDate::from_ymd_opt(2026, 4, 4).unwrap()));
        assert!(parsed.contains(&NaiveDate::from_ymd_opt(2026, 5, 9).unwrap()));
    }

    #[test]
    fn detects_existing_daily_report() {
        let snapshot = DaySnapshot {
            daily: Some(json!({"id": 123})),
            workhour: Vec::new(),
        };
        assert!(has_daily_report(&snapshot));

        let snapshot = DaySnapshot {
            daily: Some(json!({})),
            workhour: Vec::new(),
        };
        assert!(!has_daily_report(&snapshot));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn matches_reminder_windows() {
        assert!(is_between_hours(NaiveTime::from_hms_opt(8, 0, 0).unwrap(), 8, 10));
        assert!(is_between_hours(NaiveTime::from_hms_opt(9, 59, 0).unwrap(), 8, 10));
        assert!(!is_between_hours(NaiveTime::from_hms_opt(10, 0, 0).unwrap(), 8, 10));
        assert!(is_between_hours(NaiveTime::from_hms_opt(18, 0, 0).unwrap(), 18, 20));
        assert!(!is_between_hours(NaiveTime::from_hms_opt(20, 0, 0).unwrap(), 18, 20));
    }
}
