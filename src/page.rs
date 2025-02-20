use crate::{ext::RequestExt, websocket::ws_handler};
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

pub async fn router_handler(req: Request, env: Env) -> Result<Response> {
    let user_str = env.var("USER_ID").map_or(String::new(), |s| s.to_string());

    //判断是否为websocket请求
    if req
        .header("Upgrade")
        .map_or(false, |s| s.to_lowercase() == "websocket")
    {

        if let Ok(namespace)=env.durable_object("WEBSOCKETSESSION"){
            if let Ok(object)=namespace.id_from_name("WS"){
                if let Ok(stub)=object.get_stub(){
                    return stub.fetch_with_request(req).await;
                }
            }
        }

        return ws_handler(req, &env);
    }

    //判断用户UUID,如果存在则转去订阅页面
    if req.path().to_string().contains(user_str.as_str()) && user_str.len() > 0 {
        return subscribe_page(req, user_str);
    }

    default_page(env)
}
