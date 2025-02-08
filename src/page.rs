use crate::{
    ext::RequestExt,
    proxy::{parse_early_data, parse_user_id},
    websocket::ws_handler,
};
use base64::{engine::general_purpose, Engine as _};
use worker::*;

/**
 * 默认页面
 */
pub fn default_page(env: Env) -> Result<Response> {
    let html = env
        .var("DEFAULT_PAGE")
        .map_or(String::new(), |s| s.to_string());

    Response::from_html(html)
}

/**
 * 订阅页面
 */
fn subscribe_page(req: Request, user_str: String) -> Result<Response> {
    let host_str = req.url()?.host_str().unwrap().to_string();
    let body = format!(
        "vless://{uuid}@{host}:443?encryption=none&security=tls&sni={host}&fp=chrome&type=ws&host={host}&path=ws#{host}",
        uuid = user_str,
        host = host_str
    );
    Response::from_html(general_purpose::STANDARD.encode(body))
}

pub fn router_handler(req: Request, env: Env) -> Result<Response> {
    let user_str = env.var("USER_ID").map_or(String::new(), |s| s.to_string());
    let user_id = parse_user_id(&user_str);

    // get proxy ip list
    let proxy_ip = env.var("PROXY_IP").map_or(String::new(), |s| s.to_string());
    let proxy_ip = proxy_ip
        .split_ascii_whitespace()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect::<Vec<String>>();

    //判断是否为websocket请求
    let is_ws = req
        .header("Upgrade")
        .map_or(false, |s| s.to_lowercase() == "websocket");

    let ws_protocol = req.header("sec-websocket-protocol");
    let ws_protocol = parse_early_data(ws_protocol).unwrap_or(None);

    if is_ws && ws_protocol.is_some() {
        return ws_handler(user_id, proxy_ip, ws_protocol);
    }

    //判断用户UUID,如果存在则转去痛授权页面
    if req.path().to_string().contains(user_str.as_str()) && user_str.len() > 0 {
        return subscribe_page(req, user_str);
    }

    default_page(env)
}
