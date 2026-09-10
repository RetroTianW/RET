//! dirscan —— 基于字典的并发目录扫描工具
//!
//! 功能：
//! - 从字典文件读取路径列表并发扫描
//! - 实时输出每个路径的 HTTP 状态码
//! - 遇到 3xx 重定向时显示跳转目标，并将同源目标自动加入扫描队列（去重）
//! - 显示响应字节大小（Content-Length），支持按状态码过滤（-f）与排除（-e）输出记录
//! - 支持 HTTP/HTTPS/SOCKS5 代理（可多代理轮换、内嵌认证、绕过本地地址），带失败重试
//! - 支持并发数、超时时间、User-Agent 等参数配置

use std::collections::{HashSet, VecDeque};
use std::ops::RangeInclusive;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use clap::Parser;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, LOCATION, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{Client, Url};

// ---- ANSI 颜色 ----
const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";

/// 按状态码返回颜色
fn color_for(code: u16) -> &'static str {
    match code {
        200..=299 => GREEN,
        300..=399 => CYAN,
        400..=499 => YELLOW,
        _ => RED,
    }
}

/// 状态码过滤器：精确码或闭区间列表（如 200、301、400-499）
#[derive(Debug, Clone)]
struct StatusFilter {
    ranges: Vec<RangeInclusive<u16>>,
}

impl StatusFilter {
    /// 解析 "200,301,400-499" 形式的表达式；what 用于错误消息（"过滤"/"排除"）
    fn parse(spec: &str, what: &str) -> Result<Self, String> {
        let mut ranges = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let range = match part.split_once('-') {
                Some((lo, hi)) => {
                    let lo: u16 = lo
                        .trim()
                        .parse()
                        .map_err(|_| format!("无效的状态码{what}项 '{part}'"))?;
                    let hi: u16 = hi
                        .trim()
                        .parse()
                        .map_err(|_| format!("无效的状态码{what}项 '{part}'"))?;
                    if lo > hi {
                        return Err(format!("{what}区间下界大于上界: '{part}'"));
                    }
                    lo..=hi
                }
                None => {
                    let code: u16 = part
                        .parse()
                        .map_err(|_| format!("无效的状态码{what}项 '{part}'"))?;
                    code..=code
                }
            };
            ranges.push(range);
        }
        if ranges.is_empty() {
            return Err(format!("状态码{what}表达式 '{spec}' 不包含任何有效项"));
        }
        Ok(Self { ranges })
    }

    /// 判断状态码是否匹配
    fn matches(&self, code: u16) -> bool {
        self.ranges.iter().any(|r| r.contains(&code))
    }
}

/// 结果显示策略：包含列表（-f）与排除列表（-e）可独立或组合使用，排除优先
#[derive(Debug, Clone, Default)]
struct Visibility {
    /// 白名单：仅匹配的状态码显示；None 表示不限制
    include: Option<StatusFilter>,
    /// 黑名单：匹配的状态码一律隐藏，优先于白名单
    exclude: Option<StatusFilter>,
}

impl Visibility {
    /// 判断状态码是否显示：先查排除（命中即隐藏），再查包含（未匹配即隐藏）
    fn is_visible(&self, code: u16) -> bool {
        if self.exclude.as_ref().is_some_and(|e| e.matches(code)) {
            return false;
        }
        self.include.as_ref().map_or(true, |f| f.matches(code))
    }

    /// 是否启用了任意显示限制（影响统计注脚与 ERR 行为）
    fn any(&self) -> bool {
        self.include.is_some() || self.exclude.is_some()
    }
}

/// 解析可选的状态码表达式（-f/-e 共用）
fn parse_optional_filter(spec: Option<&str>, what: &str) -> Result<Option<StatusFilter>, String> {
    spec.map(|s| StatusFilter::parse(s, what)).transpose()
}

/// 将响应字节数格式化为显示文本；无 Content-Length 时显示 "-"
fn format_size(size: Option<u64>) -> String {
    match size {
        Some(n) => format!("{n}B"),
        None => "-".to_string(),
    }
}

#[derive(Parser, Debug)]
#[command(name = "dirscan", version, about = "基于字典的并发目录扫描工具")]
struct Args {
    /// 目标基础 URL，例如 https://example.com/ 或 https://example.com/app/
    #[arg(short, long)]
    url: String,

    /// 字典文件路径（每行一个路径，# 开头为注释）
    #[arg(short, long)]
    wordlist: String,

    /// 并发任务数
    #[arg(short, long, default_value_t = 10)]
    threads: usize,

    /// 单个请求超时时间（秒）
    #[arg(short = 'T', long, default_value_t = 10)]
    timeout: u64,

    /// 只显示匹配的状态码记录，支持精确码与区间，逗号分隔，如 200,301,400-499
    #[arg(short = 'f', long)]
    filter: Option<String>,

    /// 排除匹配的状态码记录，语法与 -f 相同，如 404,500-599；可与 -f 组合，排除优先
    #[arg(short = 'e', long)]
    exclude: Option<String>,

    /// User-Agent 列表：逗号分隔或重复传参；多个时请求间轮换
    #[arg(long, value_delimiter = ',', default_values = ["dirscan/0.1"])]
    user_agent: Vec<String>,

    /// Cookie：可重复传参，或一次传入分号拼接的完整串；如 --cookie "a=1" --cookie "b=2"
    #[arg(long, value_delimiter = ';')]
    cookie: Vec<String>,

    /// HTTP Basic 认证凭据，格式 user:pass（密码可含冒号）
    #[arg(long)]
    auth: Option<String>,

    /// 代理列表：支持 http://、https://、socks5://、socks5h://，可内嵌认证 user:pass@host:port；逗号分隔或重复传参，多个时请求间轮换；未指定时读取 ALL_PROXY/HTTP(S)_PROXY 环境变量
    #[arg(long, value_delimiter = ',', value_name = "URL")]
    proxy: Vec<String>,

    /// 代理列表文件（每行一个代理 URL，# 开头为注释）
    #[arg(long, value_name = "FILE")]
    proxy_file: Option<String>,

    /// 代理绕过列表：命中的主机直连不走代理（逗号分隔）；默认 localhost,127.0.0.1,::1，另读取 NO_PROXY 环境变量
    #[arg(long, value_delimiter = ',', value_name = "LIST")]
    no_proxy: Option<String>,

    /// 请求失败（超时/连接错误）时的重试次数；配置多个代理时重试自动切换下一个代理
    #[arg(short = 'r', long, default_value_t = 1)]
    retries: u32,
}

/// 扫描统计
#[derive(Default)]
struct Stats {
    total: AtomicU64,
    found: AtomicU64,
    redirects: AtomicU64,
    errors: AtomicU64,
    shown: AtomicU64,
    retried: AtomicU64,
}

/// worker 间共享的状态
struct Shared {
    queue: Mutex<VecDeque<Url>>,
    seen: Mutex<HashSet<String>>,
    pending: AtomicU64,
    stats: Stats,
    /// User-Agent 池（至少 1 个）；多于 1 个时按 round-robin 轮换
    ua_pool: Vec<String>,
    ua_next: AtomicU64,
    /// 客户端池轮换索引（代理多于 1 个时 round-robin）
    client_next: AtomicU64,
}

/// 从 UA 池中轮换取下一个 User-Agent（round-robin；仅 1 个时直接返回，避免原子操作）
fn next_user_agent(shared: &Shared) -> &str {
    if shared.ua_pool.len() == 1 {
        return &shared.ua_pool[0];
    }
    let idx = (shared.ua_next.fetch_add(1, Ordering::Relaxed) as usize) % shared.ua_pool.len();
    &shared.ua_pool[idx]
}

/// 从客户端池中选出本次请求使用的客户端（round-robin；仅 1 个时直接返回）
///
/// `attempt` 为当前重试轮次（0 起），多代理时重试会偏移到池中下一个代理。
fn pick_client<'a>(pool: &'a [Client], next: &AtomicU64, attempt: u32) -> &'a Client {
    if pool.len() == 1 {
        return &pool[0];
    }
    let base = next.fetch_add(1, Ordering::Relaxed) as usize;
    &pool[(base + attempt as usize) % pool.len()]
}

/// 将 URL 加入扫描队列；若已扫描过/已在队列则返回 false（去重）
fn enqueue(shared: &Shared, url: Url) -> bool {
    let mut seen = shared
        .seen
        .lock()
        .expect("seen 锁中毒（其他线程 panic）");
    if seen.insert(url.as_str().to_string()) {
        drop(seen);
        shared
            .queue
            .lock()
            .expect("queue 锁中毒（其他线程 panic）")
            .push_back(url);
        true
    } else {
        false
    }
}

/// 解析并校验基础 URL；确保路径以 '/' 结尾以保证相对路径拼接正确
fn parse_base_url(raw: &str) -> Result<Url, String> {
    let mut base = Url::parse(raw).map_err(|e| format!("无效的目标 URL '{raw}': {e}"))?;
    if !matches!(base.scheme(), "http" | "https") {
        return Err(format!("不支持的协议 '{}'（仅支持 http/https）", base.scheme()));
    }
    if !base.path().ends_with('/') {
        let mut path = base.path().to_string();
        path.push('/');
        base.set_path(&path);
    }
    Ok(base)
}

/// 读取字典文件，返回清洗后的路径列表（去空白、跳过注释、去掉开头的 '/'）
fn load_wordlist(path: &str) -> Result<Vec<String>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("无法读取字典文件 '{path}': {e}"))?;
    let words: Vec<String> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.trim_start_matches('/').to_string())
        .collect();
    if words.is_empty() {
        return Err(format!("字典文件 '{path}' 为空或只包含注释"));
    }
    Ok(words)
}

/// 解析 "user:pass" 形式的 Basic 认证凭据
///
/// 按第一个冒号拆分，因此密码中可以包含冒号；用户名不允许为空。
fn parse_basic_auth(spec: &str) -> Result<(String, String), String> {
    match spec.split_once(':') {
        Some((user, pass)) if !user.is_empty() => Ok((user.to_string(), pass.to_string())),
        _ => Err(format!("无效的认证凭据 '{spec}'（正确格式: user:pass）")),
    }
}

/// 将 reqwest 错误归类为简短中文原因标签（超时/连接失败/请求错误）
fn error_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "超时"
    } else if e.is_connect() {
        "连接失败"
    } else {
        "请求错误"
    }
}

/// 规范化代理 URL：无 scheme 时默认补 http:// 前缀（如 127.0.0.1:8080）
fn normalize_proxy_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    }
}

/// 返回代理类型标签（HTTP/HTTPS/SOCKS5/SOCKS5H）；不支持的协议报错
fn proxy_kind(url: &str) -> Result<&'static str, String> {
    let lower = url.to_ascii_lowercase();
    let kind = if lower.starts_with("socks5h://") {
        "SOCKS5H"
    } else if lower.starts_with("socks5://") {
        "SOCKS5"
    } else if lower.starts_with("https://") {
        "HTTPS"
    } else if lower.starts_with("http://") {
        "HTTP"
    } else {
        return Err(format!(
            "不支持的代理协议 '{url}'（支持 http://、https://、socks5://、socks5h://）"
        ));
    };
    Ok(kind)
}

/// 读取代理列表文件（每行一个代理 URL，# 开头为注释，自动补 http:// 前缀）
fn load_proxy_file(path: &str) -> Result<Vec<String>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("无法读取代理文件 '{path}': {e}"))?;
    let urls: Vec<String> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(normalize_proxy_url)
        .collect();
    if urls.is_empty() {
        return Err(format!("代理文件 '{path}' 为空或只包含注释"));
    }
    Ok(urls)
}

/// 收集代理列表：--proxy/--proxy-file 优先，否则按目标协议读环境变量
///
/// 环境变量优先级：ALL_PROXY > HTTPS_PROXY/HTTP_PROXY（按目标 scheme 选择，兼顾小写变体）。
/// 返回 (代理 URL 列表, 来源描述)；无任何代理时列表为空。
fn collect_proxies(args: &Args, base: &Url) -> Result<(Vec<String>, String), String> {
    let mut from_args: Vec<String> = args
        .proxy
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| normalize_proxy_url(&s))
        .collect();
    let mut source = if args.proxy.is_empty() {
        String::new()
    } else {
        "--proxy".to_string()
    };
    if let Some(path) = args.proxy_file.as_deref() {
        from_args.extend(load_proxy_file(path)?);
        if !source.is_empty() {
            source.push('/');
        }
        source.push_str("--proxy-file");
    }
    if !from_args.is_empty() {
        return Ok((from_args, source));
    }

    // 环境变量兜底：大小写变体按序探测（Windows 下环境变量不区分大小写，重复探测无害）
    let candidates: [&str; 4] = if base.scheme() == "https" {
        ["ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy"]
    } else {
        ["ALL_PROXY", "all_proxy", "HTTP_PROXY", "http_proxy"]
    };
    for name in candidates {
        if let Ok(v) = std::env::var(name) {
            let v = v.trim();
            if v.is_empty() {
                continue;
            }
            return Ok((vec![normalize_proxy_url(v)], format!("环境变量 {name}")));
        }
    }
    Ok((Vec::new(), String::new()))
}

/// 合并代理绕过列表：默认本地地址 + --no-proxy 参数（或 NO_PROXY 环境变量）
fn merge_no_proxy(cli: Option<&str>) -> String {
    const DEFAULT_BYPASS: &str = "localhost,127.0.0.1,::1";
    let extra = match cli.map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => std::env::var("NO_PROXY").unwrap_or_default().trim().to_string(),
    };
    if extra.is_empty() {
        DEFAULT_BYPASS.to_string()
    } else {
        format!("{DEFAULT_BYPASS},{extra}")
    }
}

/// 轻量判断目标 host 是否命中绕过列表（精确匹配或域名后缀匹配，仅用于日志措辞）
fn host_bypassed(host: &str, list: &str) -> bool {
    list.split(',').map(str::trim).any(|entry| {
        let entry = entry.trim_start_matches('.');
        !entry.is_empty()
            && (host == entry
                || host.strip_suffix(entry).is_some_and(|p| p.ends_with('.')))
    })
}

fn build_client(
    timeout: u64,
    cookies: &[String],
    auth: Option<&(String, String)>,
    proxy: Option<(&str, Option<&reqwest::NoProxy>)>,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .redirect(Policy::none()) // 不自动跟随重定向，由本工具自行处理
        .timeout(Duration::from_secs(timeout))
        .connect_timeout(Duration::from_secs(timeout));

    // 代理统一在此配置：认证信息内嵌于 URL（user:pass@host:port），由 reqwest 解析
    match proxy {
        Some((purl, no_proxy)) => {
            let mut px =
                reqwest::Proxy::all(purl).map_err(|e| format!("无效的代理 '{purl}': {e}"))?;
            if let Some(np) = no_proxy {
                px = px.no_proxy(Some(np.clone()));
            }
            builder = builder.proxy(px);
        }
        None => {
            // 显式禁用系统级隐式代理（环境变量/注册表），保证代理来源完全可控：
            // 需要代理时由 collect_proxies 显式收集并打日志
            builder = builder.no_proxy();
        }
    }

    let mut default_headers = HeaderMap::new();

    // Cookie：多条以 "; " 拼接为一个 Cookie 头，作为客户端默认头注入所有请求
    if !cookies.is_empty() {
        let joined = cookies.join("; ");
        let value = HeaderValue::from_str(&joined)
            .map_err(|e| format!("Cookie 含非法字符: {e}"))?;
        default_headers.insert(HeaderName::from_static("cookie"), value);
    }

    // Basic 认证：Authorization: Basic base64(user:pass)，同样以默认头注入
    if let Some((user, pass)) = auth {
        let encoded = BASE64.encode(format!("{user}:{pass}"));
        let value = HeaderValue::from_str(&format!("Basic {encoded}"))
            .map_err(|e| format!("认证凭据含非法字符: {e}"))?;
        default_headers.insert(HeaderName::from_static("authorization"), value);
    }

    if !default_headers.is_empty() {
        builder = builder.default_headers(default_headers);
    }

    builder
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败: {e}"))
}

/// 打印普通（非重定向）响应结果
fn print_status(url: &Url, code: u16, size: Option<u64>) {
    println!(
        "[{c}{code}{RESET}] {url} [{s}]",
        c = color_for(code),
        url = url.as_str(),
        s = format_size(size)
    );
}

/// 处理 3xx 重定向：同源目标无条件入队（去重），仅在 visible 时打印跳转目标与大小
fn handle_redirect(
    shared: &Shared,
    url: &Url,
    base: &Url,
    code: u16,
    size: Option<u64>,
    headers: &HeaderMap,
    visible: bool,
) {
    let s = format_size(size);
    // Location 相对路径应基于“当前请求 URL”解析，而非基础 URL；
    // 入队逻辑与显示无关，即使该行被状态码过滤也保持递归发现能力
    let (note, target) = match headers.get(LOCATION).and_then(|v| v.to_str().ok()) {
        None => ("(响应缺少有效的 Location 头)".to_string(), None),
        Some(loc) => match url.join(loc) {
            Ok(target) => {
                let same_origin = target.host_str() == base.host_str()
                    && target.port_or_known_default() == base.port_or_known_default();
                let note = if !same_origin {
                    "(跨域跳转，不入队)"
                } else if enqueue(shared, target.clone()) {
                    "(已加入扫描队列)"
                } else {
                    "(已在队列中，跳过)"
                };
                (note.to_string(), Some(target.to_string()))
            }
            Err(e) => (format!("(无效的 Location '{loc}': {e})"), None),
        },
    };

    if !visible {
        return;
    }
    match target {
        Some(t) => println!(
            "[{CYAN}{code}{RESET}] {url} [{s}] -> {CYAN}{t}{RESET} {note}",
            url = url.as_str()
        ),
        None => println!(
            "[{CYAN}{code}{RESET}] {url} [{s}] -> {note}",
            url = url.as_str()
        ),
    }
}

/// 单个 worker：循环从队列取 URL 扫描，直到队列空且无进行中的任务
///
/// 请求失败（超时/连接错误）时按 `retries` 重试；客户端池多于 1 个（多代理）时
/// 每次重试自动切换池中下一个客户端（代理）。
async fn worker(
    shared: Arc<Shared>,
    clients: Vec<Client>,
    base: Url,
    vis: Visibility,
    retries: u32,
) {
    loop {
        // 取任务（锁的作用域在 await 之前结束）
        let popped = {
            let mut queue = shared.queue.lock().expect("queue 锁中毒");
            queue.pop_front()
        };
        let url = match popped {
            Some(u) => {
                shared.pending.fetch_add(1, Ordering::AcqRel);
                u
            }
            None => {
                if shared.pending.load(Ordering::Acquire) == 0 {
                    return; // 队列空且无在途任务，结束
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            }
        };

        shared.stats.total.fetch_add(1, Ordering::Relaxed);

        // 每请求注入 UA（单个时为固定值，多个时 round-robin 轮换）；
        // 请求失败且可重试（超时/连接错误）时切换池中下一个客户端再试
        let ua = next_user_agent(&shared);
        let mut attempt = 0u32;
        let result = loop {
            let client = pick_client(&clients, &shared.client_next, attempt);
            match client.get(url.clone()).header(USER_AGENT, ua).send().await {
                Ok(resp) => break Ok(resp),
                Err(e) => {
                    let retryable = e.is_timeout() || e.is_connect();
                    if retryable && attempt < retries {
                        attempt += 1;
                        shared.stats.retried.fetch_add(1, Ordering::Relaxed);
                        println!(
                            "[i] 重试 {attempt}/{retries}: {url} ({kind})",
                            kind = error_kind(&e),
                            url = url.as_str()
                        );
                        continue;
                    }
                    break Err(e);
                }
            }
        };
        match result {
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                let size = resp.content_length();
                let visible = vis.is_visible(code);
                if status.is_redirection() {
                    shared.stats.redirects.fetch_add(1, Ordering::Relaxed);
                    handle_redirect(&shared, &url, &base, code, size, resp.headers(), visible);
                    if visible {
                        shared.stats.shown.fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    if visible {
                        shared.stats.shown.fetch_add(1, Ordering::Relaxed);
                        print_status(&url, code, size);
                    }
                    if code < 300 {
                        shared.stats.found.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Err(e) => {
                shared.stats.errors.fetch_add(1, Ordering::Relaxed);
                // 白名单（-f）存在时 ERR 无法匹配而隐藏；仅排除（-e）时 ERR 仍显示
                if vis.include.is_none() {
                    println!(
                        "[{MAGENTA}ERR{RESET}] {url} - {reason}: {e}",
                        reason = error_kind(&e),
                        url = url.as_str()
                    );
                }
            }
        }
        shared.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 预检：逐个用客户端池（直连或各代理）访问目标，任一成功即通过
///
/// 返回 (成功客户端索引, 耗时)；全部失败时返回汇总错误（配置代理时附提示）。
async fn preflight(
    clients: &[Client],
    proxies: &[String],
    base: &Url,
    ua: &str,
) -> Result<(usize, Duration), String> {
    let mut last_err = String::new();
    for (i, client) in clients.iter().enumerate() {
        let via = proxies.get(i).map(String::as_str).unwrap_or("直连");
        let t0 = Instant::now();
        match client.get(base.clone()).header(USER_AGENT, ua).send().await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if code >= 500 {
                    eprintln!("[!] 警告：目标返回 {code}，仍继续扫描");
                }
                return Ok((i, t0.elapsed()));
            }
            Err(e) => {
                last_err = format!("({via}) {kind}: {e}", kind = error_kind(&e));
            }
        }
    }
    let hint = if proxies.is_empty() {
        ""
    } else {
        "（已配置代理，请检查代理可用性）"
    };
    Err(format!("目标 URL 不可达: {last_err}{hint}"))
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let start = Instant::now();

    // 1. 解析并校验基础 URL
    let base = match parse_base_url(&args.url) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[!] {e}");
            return ExitCode::FAILURE;
        }
    };

    // 2. 读取字典
    let words = match load_wordlist(&args.wordlist) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[!] {e}");
            return ExitCode::FAILURE;
        }
    };

    // 3. 解析状态码显示规则：-f 包含 + -e 排除（可组合，排除优先）
    let visibility = match (
        parse_optional_filter(args.filter.as_deref(), "过滤"),
        parse_optional_filter(args.exclude.as_deref(), "排除"),
    ) {
        (Ok(include), Ok(exclude)) => Visibility { include, exclude },
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("[!] {e}");
            return ExitCode::FAILURE;
        }
    };

    // 4. 清洗 UA 列表（去空白/空项；--user-agent "," 会拆出空串）
    let ua_pool: Vec<String> = args
        .user_agent
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ua_pool.is_empty() {
        eprintln!("[!] User-Agent 列表为空");
        return ExitCode::FAILURE;
    }

    // 5. 清洗 Cookie 列表（分号拆分后可能残留空白前缀）
    let cookies: Vec<String> = args
        .cookie
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();

    // 6. 解析 Basic 认证凭据（user:pass）
    let auth = match args.auth.as_deref() {
        Some(spec) => match parse_basic_auth(spec) {
            Ok(pair) => Some(pair),
            Err(e) => {
                eprintln!("[!] {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    // 7. 收集代理（--proxy/--proxy-file > 环境变量），构建客户端池（每个代理一个客户端）
    let (proxy_urls, proxy_source) = match collect_proxies(&args, &base) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[!] {e}");
            return ExitCode::FAILURE;
        }
    };
    // 校验每个代理的协议合法性（http/https/socks5/socks5h），失败快速退出
    let kinds: Vec<&str> = match proxy_urls.iter().map(|u| proxy_kind(u)).collect() {
        Ok(ks) => ks,
        Err(e) => {
            eprintln!("[!] {e}");
            return ExitCode::FAILURE;
        }
    };
    // 绕过列表：默认本地地址 + --no-proxy/NO_PROXY；所有客户端共享同一份解析结果
    let no_proxy_text = merge_no_proxy(args.no_proxy.as_deref());
    let parsed_no_proxy = reqwest::NoProxy::from_string(&no_proxy_text);

    let mut pool = Vec::with_capacity(proxy_urls.len().max(1));
    if proxy_urls.is_empty() {
        match build_client(args.timeout, &cookies, auth.as_ref(), None) {
            Ok(c) => pool.push(c),
            Err(e) => {
                eprintln!("[!] {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        for purl in &proxy_urls {
            match build_client(
                args.timeout,
                &cookies,
                auth.as_ref(),
                Some((purl, parsed_no_proxy.as_ref())),
            ) {
                Ok(c) => pool.push(c),
                Err(e) => {
                    eprintln!("[!] {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    // 8. 预检目标可达性（逐客户端测试：直连或各代理；任一成功即通过）
    let (ok_idx, elapsed) = match preflight(&pool, &proxy_urls, &base, &ua_pool[0]).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[!] {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(via) = proxy_urls.get(ok_idx) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        let note = if host_bypassed(base.host_str().unwrap_or(""), &no_proxy_text) {
            format!("目标命中代理绕过列表，直连测试成功 ({ms:.0}ms)")
        } else {
            format!("代理连接测试: 通过 {kind} 代理 {via} 访问目标成功 ({ms:.0}ms)", kind = kinds[ok_idx])
        };
        println!("[i] {note}");
    }

    let threads = args.threads.max(1);
    println!(
        "开始扫描: {target} | 字典: {n} 条 | 并发: {threads} | 超时: {t}s | 过滤: {f} | 排除: {e}",
        target = base.as_str(),
        n = words.len(),
        t = args.timeout,
        f = args.filter.as_deref().unwrap_or("全部"),
        e = args.exclude.as_deref().unwrap_or("无")
    );
    println!("{}", "-".repeat(60));
    // 非默认请求配置摘要，便于确认 UA/Cookie/认证已生效
    let mut notes = Vec::new();
    if ua_pool.len() > 1 {
        notes.push(format!("UA 池: {} 个（轮换）", ua_pool.len()));
    }
    if !cookies.is_empty() {
        notes.push(format!("Cookie: {} 条", cookies.len()));
    }
    if let Some((user, _)) = &auth {
        notes.push(format!("Basic 认证: 用户 '{user}'"));
    }
    if !proxy_urls.is_empty() {
        // 代理摘要：类型×数量 + 认证标记 + 绕过列表 + 来源
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for k in &kinds {
            *counts.entry(k).or_default() += 1;
        }
        let has_auth = proxy_urls
            .iter()
            .any(|u| u.split_once("://").is_some_and(|(_, r)| r.contains('@')));
        let summary = counts
            .iter()
            .map(|(k, n)| format!("{k}×{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let auth_note = if has_auth { " [含认证]" } else { "" };
        notes.push(format!(
            "代理: {n} 个（轮换）[{summary}]{auth_note} | 绕过: {no_proxy_text} | 来源: {proxy_source}",
            n = proxy_urls.len()
        ));
        if args.retries > 0 {
            notes.push(format!("失败重试: {} 次/请求", args.retries));
        }
    }
    if !notes.is_empty() {
        println!("[i] {}", notes.join(" | "));
    }

    // 8. 初始化队列（seen 集合自动去重）
    let shared = Arc::new(Shared {
        queue: Mutex::new(VecDeque::with_capacity(words.len())),
        seen: Mutex::new(HashSet::with_capacity(words.len() * 2)),
        pending: AtomicU64::new(0),
        stats: Stats::default(),
        ua_pool,
        ua_next: AtomicU64::new(0),
        client_next: AtomicU64::new(0),
    });
    for word in &words {
        if let Ok(u) = base.join(word) {
            enqueue(&shared, u);
        }
    }

    // 启动并发 worker（每 worker 持有完整客户端池；Client clone 仅复制内部 Arc）
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let shared = Arc::clone(&shared);
        let clients = pool.clone();
        let base = base.clone();
        let vis = visibility.clone();
        handles.push(tokio::spawn(worker(shared, clients, base, vis, args.retries)));
    }
    for h in handles {
        let _ = h.await;
    }

    // 9. 输出统计
    println!("{}", "-".repeat(60));
    let total = shared.stats.total.load(Ordering::Relaxed);
    let found = shared.stats.found.load(Ordering::Relaxed);
    let redirects = shared.stats.redirects.load(Ordering::Relaxed);
    let errors = shared.stats.errors.load(Ordering::Relaxed);
    let shown = shared.stats.shown.load(Ordering::Relaxed);
    let retried = shared.stats.retried.load(Ordering::Relaxed);
    let mut tail = String::new();
    if visibility.any() {
        tail.push_str(&format!(" | 符合过滤并显示: {shown}"));
    }
    if retried > 0 {
        tail.push_str(&format!(" | 重试: {retried}"));
    }
    println!(
        "扫描完成，耗时 {elapsed:.1}s | 总请求: {total} | 2xx 发现: {GREEN}{found}{RESET} | 重定向: {redirects} | 错误: {RED}{errors}{RESET}{tail}",
        elapsed = start.elapsed().as_secs_f64(),
    );

    ExitCode::SUCCESS
}
