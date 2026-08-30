//! Lokaler, token-geschuetzter HTTP-Viewer fuer die texturierte Live-POV.
//!
//! Kein Framework und kein CDN: HTML, CSS und JavaScript kommen aus der Binary, Frames und
//! Zustandsdaten nur aus dem laufenden Client. Dadurch funktioniert die Ansicht offline und ein
//! fremder Webseiten-Tab kann nicht blind das lokale Menue anklicken.

use crate::client::Shared;
use rand::RngCore;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MAX_REQUEST: usize = 16 * 1024;
const WEB_WORKERS: usize = 4;
const CONNECTION_QUEUE: usize = 8;

pub(crate) fn start(shared: &Arc<Shared>, address: SocketAddr) -> Result<(), String> {
    let listener = TcpListener::bind(address)
        .map_err(|error| format!("{} kann nicht geoeffnet werden: {}", address, error))?;
    let token = access_token();
    let shown_host = if address.ip().is_unspecified() {
        if address.is_ipv6() {
            "[::1]".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    } else if address.is_ipv6() {
        // Die Klammern gehoeren in eine URL, `SocketAddr::ip()` liefert sie nicht.
        format!("[{}]", address.ip())
    } else {
        address.ip().to_string()
    };
    let url = format!("http://{}:{}/?token={}", shown_host, address.port(), token);
    shared.console.info(&format!("Browser-POV: {}", url));
    if !address.ip().is_loopback() {
        shared.console.warn(
            "Browser-POV lauscht ausserhalb von localhost. Die URL enthaelt den Zugriffstoken; nicht weitergeben.",
        );
    }
    if let Some(error) = shared.extras.pov.asset_error() {
        shared
            .console
            .warn(&format!("Browser-POV startet ohne Texturen: {}", error));
    }

    let owned = Arc::clone(shared);
    thread::Builder::new()
        .name("afk-pov-web".into())
        .spawn(move || {
            serve(listener, owned, token);
        })
        .map_err(|error| format!("HTTP-Thread liess sich nicht starten: {}", error))?;
    Ok(())
}

fn access_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

fn serve(listener: TcpListener, shared: Arc<Shared>, token: String) {
    if let Err(error) = listener.set_nonblocking(true) {
        shared.console.error(&format!(
            "Browser-POV konnte nicht gestartet werden: {}",
            error
        ));
        return;
    }

    let (sender, receiver) = sync_channel::<TcpStream>(CONNECTION_QUEUE);
    let receiver = Arc::new(Mutex::new(receiver));
    let token: Arc<str> = token.into();
    let mut workers = Vec::with_capacity(WEB_WORKERS);
    for index in 0..WEB_WORKERS {
        let receiver = Arc::clone(&receiver);
        let worker_shared = Arc::clone(&shared);
        let token = Arc::clone(&token);
        match thread::Builder::new()
            .name(format!("afk-pov-http-{}", index + 1))
            .spawn(move || loop {
                let stream = {
                    let Ok(receiver) = receiver.lock() else {
                        return;
                    };
                    receiver.recv()
                };
                match stream {
                    Ok(stream) => handle(stream, &worker_shared, &token),
                    Err(_) => return,
                }
            }) {
            Ok(worker) => workers.push(worker),
            Err(error) => shared.console.warn(&format!(
                "Browser-POV: HTTP-Arbeiter konnte nicht starten: {}",
                error
            )),
        }
    }
    if workers.is_empty() {
        shared
            .console
            .error("Browser-POV: Kein HTTP-Arbeiter konnte gestartet werden");
        return;
    }

    while shared.running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => match sender.try_send(stream) {
                Ok(()) => {}
                Err(TrySendError::Full(mut stream)) => {
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                    response(
                        &mut stream,
                        503,
                        "text/plain; charset=utf-8",
                        b"Browser-POV ist ausgelastet",
                    );
                }
                Err(TrySendError::Disconnected(_)) => break,
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                shared.console.warn(&format!(
                    "Browser-POV: Verbindung fehlgeschlagen: {}",
                    error
                ));
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    drop(sender);
    for worker in workers {
        let _ = worker.join();
    }
}

fn handle(mut stream: TcpStream, shared: &Arc<Shared>, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(()) => {
            return response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                b"Ungueltige oder zu grosse HTTP-Anfrage",
            )
        }
    };
    let Some(first) = request.lines().next() else {
        return;
    };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("");
    if parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return response(
            &mut stream,
            400,
            "text/plain; charset=utf-8",
            b"Ungueltige HTTP-Anfrage",
        );
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let authorized = query_value(query, "token") == Some(token);
    if !authorized {
        return response(
            &mut stream,
            403,
            "text/plain; charset=utf-8",
            b"Zugriffstoken fehlt. Die vollstaendige URL steht im Terminal des AFK-Clients.",
        );
    }

    if method == "GET" && path.starts_with("/assets/") {
        let resource = path.trim_start_matches('/');
        if !resource.ends_with(".png") || resource.contains("..") || resource.contains('\\') {
            return response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                b"Ungueltiger Ressourcenpfad",
            );
        }
        let Some(bytes) =
            crate::pov::web_asset(shared, resource).or_else(|| crate::pov::web_missing(shared))
        else {
            return response(
                &mut stream,
                503,
                "text/plain; charset=utf-8",
                b"Keine Minecraft-Ressourcen geladen",
            );
        };
        return immutable_response(&mut stream, 200, "image/png", &bytes);
    }

    match (method, path) {
        ("GET", "/") => {
            let page = PAGE.replace("__TOKEN__", token);
            response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
            );
        }
        ("GET", "/api/state.json") => {
            let mut value = crate::pov::web_world(shared);
            if let Some(object) = value.as_object_mut() {
                #[cfg(feature = "menu")]
                object.insert("menu".to_string(), crate::menu::web_state(shared));
                #[cfg(not(feature = "menu"))]
                object.insert(
                    "menu".to_string(),
                    serde_json::json!({"open": false, "inventory": []}),
                );
                #[cfg(feature = "state")]
                object.insert(
                    "selected_hotbar".to_string(),
                    shared.extras.selected_hotbar.load(Ordering::Relaxed).into(),
                );
                #[cfg(not(feature = "state"))]
                object.insert("selected_hotbar".to_string(), 0.into());
            }
            match serde_json::to_vec(&value) {
                Ok(bytes) => response(&mut stream, 200, "application/json", &bytes),
                Err(_) => response(&mut stream, 500, "application/json", b"{}"),
            }
        }
        ("GET", "/api/frame.png") => {
            let width = query_value(query, "w")
                .and_then(|value| value.parse().ok())
                .unwrap_or(426);
            let height = query_value(query, "h")
                .and_then(|value| value.parse().ok())
                .unwrap_or(240);
            match crate::pov::web_frame(shared, width, height) {
                Ok(bytes) => response(&mut stream, 200, "image/png", &bytes),
                Err(error) => response(
                    &mut stream,
                    503,
                    "text/plain; charset=utf-8",
                    error.as_bytes(),
                ),
            }
        }
        #[cfg(feature = "items")]
        ("GET", "/api/item.png") => {
            let id = query_value(query, "id")
                .and_then(|value| value.parse().ok())
                .unwrap_or(-1);
            let Some(bytes) =
                crate::pov::web_item(shared, id).or_else(|| crate::pov::web_missing(shared))
            else {
                return response(
                    &mut stream,
                    503,
                    "text/plain; charset=utf-8",
                    b"Keine Minecraft-Ressourcen geladen",
                );
            };
            immutable_response(&mut stream, 200, "image/png", &bytes);
        }
        #[cfg(feature = "menu")]
        ("POST", "/api/click") => {
            let slot = query_value(query, "slot").unwrap_or("");
            let Some(action) = click_action(query) else {
                return response(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    b"Ungueltige Klickart",
                );
            };
            let status = if crate::menu::click_command(shared, &format!("{} {}", slot, action)) {
                204
            } else {
                400
            };
            response(&mut stream, status, "text/plain", b"");
        }
        #[cfg(feature = "menu")]
        ("POST", "/api/close") => {
            let status = if crate::menu::close_command(shared) {
                204
            } else {
                400
            };
            response(&mut stream, status, "text/plain", b"");
        }
        #[cfg(feature = "state")]
        ("POST", "/api/hotbar") => {
            let slot = query_value(query, "slot").and_then(|value| value.parse::<usize>().ok());
            let status = match slot {
                Some(slot) if crate::extras::web_hotbar(shared, slot) => 204,
                _ => 400,
            };
            response(&mut stream, status, "text/plain", b"");
        }
        _ => response(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"Nicht gefunden",
        ),
    }
}

fn read_request(stream: &mut TcpStream) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while bytes.len() < MAX_REQUEST {
        let remaining = MAX_REQUEST - bytes.len();
        let read_len = remaining.min(chunk.len());
        let count = stream.read(&mut chunk[..read_len]).map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            return String::from_utf8(bytes).map_err(|_| ());
        }
    }
    Err(())
}

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

#[cfg(feature = "menu")]
fn click_action(query: &str) -> Option<&'static str> {
    match query_value(query, "action") {
        None | Some("left") => Some("links"),
        Some("right") => Some("rechts"),
        Some("shift") => Some("shift"),
        Some(_) => None,
    }
}

fn response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    response_with_cache(stream, status, content_type, body, "no-store");
}

fn immutable_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    response_with_cache(
        stream,
        status,
        content_type,
        body,
        "private, max-age=31536000, immutable",
    );
}

fn response_with_cache(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    cache_control: &str,
) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Resource-Policy: same-origin\r\nPermissions-Policy: camera=(), microphone=(), geolocation=()\r\nContent-Security-Policy: default-src 'self'; img-src 'self' blob:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\nConnection: close\r\n\r\n",
        status,
        reason,
        content_type,
        body.len(),
        cache_control
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// Bewusst ohne Bibliothek oder fremde Assets. Alle `background-image`-URLs zeigen in die vom
/// Nutzer bereitgestellte Original-JAR. `image-rendering: pixelated` entspricht der
/// naechstgelegenen Skalierung des Spiels fuer GUI-Texturen.
const PAGE: &str = r###"<!doctype html>
<html lang="de"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>AFKSystems – Live-POV</title>
<style>
:root{--gui:3;--shadow:#3f3f3f}*{box-sizing:border-box}html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#000;color:#fff;font-family:monospace}button{font:inherit}
#game{position:relative;width:100%;height:100%;background:#78a7ff;user-select:none}#view{width:100%;height:100%;object-fit:cover;image-rendering:pixelated;display:block}
#shade{position:absolute;inset:0;pointer-events:none;box-shadow:inset 0 0 12vw rgba(0,0,0,.38)}#crosshair{position:absolute;left:50%;top:50%;width:15px;height:15px;transform:translate(-50%,-50%);image-rendering:pixelated;filter:drop-shadow(1px 1px #000)}
#status{position:absolute;left:8px;top:7px;padding:4px 6px;background:rgba(0,0,0,.45);text-shadow:2px 2px #222;font-size:13px}#error{display:none;color:#ff5555;margin-left:8px}
#hotbar{position:absolute;left:50%;bottom:8px;width:182px;height:22px;transform:translateX(-50%) scale(var(--gui));transform-origin:bottom center;background:url('/assets/minecraft/textures/gui/sprites/hud/hotbar.png?token=__TOKEN__') 0 0/182px 22px no-repeat;image-rendering:pixelated}
#selection{position:absolute;top:-1px;width:24px;height:23px;background:url('/assets/minecraft/textures/gui/sprites/hud/hotbar_selection.png?token=__TOKEN__') 0 0/24px 23px no-repeat;image-rendering:pixelated;pointer-events:none}.hot-slot{position:absolute;top:3px;width:20px;height:18px;border:0;background:transparent;padding:1px}.hot-slot img,.slot img{width:16px;height:16px;object-fit:contain;image-rendering:pixelated}.count{position:absolute;right:0;bottom:-1px;color:#fff;text-shadow:1px 1px #3f3f3f;font:bold 8px monospace}
#menu-wrap{display:none;position:absolute;inset:0;background:rgba(0,0,0,.68);align-items:center;justify-content:center}#menu{position:relative;width:176px;transform:scale(var(--gui));image-rendering:pixelated;color:#3f3f3f;font-size:7px;text-shadow:none}.screen-bg{position:absolute;inset:0;background-repeat:no-repeat;background-position:0 0;image-rendering:pixelated}.chest-top{position:absolute;left:0;top:0;width:176px;background:url('/assets/minecraft/textures/gui/container/generic_54.png?token=__TOKEN__') 0 0/256px 256px no-repeat}.chest-bottom{position:absolute;left:0;width:176px;height:96px;background:url('/assets/minecraft/textures/gui/container/generic_54.png?token=__TOKEN__') 0 -126px/256px 256px no-repeat}.title{position:absolute;left:8px;top:6px;white-space:nowrap;overflow:hidden;width:160px}.slot{position:absolute;width:18px;height:18px;border:0;background:transparent;padding:1px}.slot:hover{background:rgba(255,255,255,.45)}#tooltip{display:none;position:absolute;z-index:20;max-width:250px;padding:6px 8px;background:rgba(16,0,16,.94);border:1px solid #2a0a35;color:#fff;text-shadow:1px 1px #222;font-size:12px;pointer-events:none;white-space:pre-line}
@media(max-width:700px),(max-height:520px){:root{--gui:2}}
</style></head><body><div id="game"><img id="view" alt="Live-POV"><div id="shade"></div><img id="crosshair" src="/assets/minecraft/textures/gui/sprites/hud/crosshair.png?token=__TOKEN__" alt=""><div id="status">Verbinde …<span id="error"></span></div><div id="hotbar"><div id="selection"></div></div><div id="menu-wrap"><div id="menu"></div></div><div id="tooltip"></div></div>
<script>
const token='__TOKEN__', api=(path)=>path+(path.includes('?')?'&':'?')+'token='+token;
const view=document.querySelector('#view'),status=document.querySelector('#status'),error=document.querySelector('#error'),hotbar=document.querySelector('#hotbar'),selection=document.querySelector('#selection'),wrap=document.querySelector('#menu-wrap'),menuEl=document.querySelector('#menu'),tooltip=document.querySelector('#tooltip');
let lastUrl='',lastHotbarKey='',lastMenuKey='';
function itemNode(item,slot,hot=false){const b=document.createElement('button');b.className=hot?'hot-slot':'slot';b.dataset.slot=slot;if(item){const img=document.createElement('img');img.src=api('/api/item.png?id='+item.id);img.alt=item.name||'';b.append(img);if(item.count>1){const c=document.createElement('span');c.className='count';c.textContent=item.count;b.append(c)}b.onmouseenter=(e)=>showTip(e,item);b.onmousemove=moveTip;b.onmouseleave=()=>tooltip.style.display='none'}return b}
function showTip(e,item){tooltip.textContent=item.name+(item.lore?.length?'\n'+item.lore.join('\n'):'');tooltip.style.display='block';moveTip(e)}function moveTip(e){tooltip.style.left=(e.clientX+14)+'px';tooltip.style.top=(e.clientY+14)+'px'}
function renderHotbar(state){const inv=state.menu?.inventory||[],key=JSON.stringify([state.selected_hotbar||0,inv.slice(36,45)]);if(key===lastHotbarKey)return;lastHotbarKey=key;hotbar.querySelectorAll('.hot-slot').forEach(n=>n.remove());for(let i=0;i<9;i++){const b=itemNode(inv[36+i]||null,i,true);b.style.left=(3+i*20)+'px';b.onclick=()=>fetch(api('/api/hotbar?slot='+i),{method:'POST'});hotbar.append(b)}selection.style.left=(-1+(state.selected_hotbar||0)*20)+'px'}
function putSlot(parent,item,index,x,y){const b=itemNode(item,index);b.style.left=x+'px';b.style.top=y+'px';b.onclick=(e)=>fetch(api('/api/click?slot='+index+'&action='+(e.shiftKey?'shift':'left')),{method:'POST'});b.oncontextmenu=(e)=>{e.preventDefault();fetch(api('/api/click?slot='+index+'&action=right'),{method:'POST'})};parent.append(b)}
const grid=(x,y,w,n)=>Array.from({length:n},(_,i)=>[x+(i%w)*18,y+Math.floor(i/w)*18]);
const layouts={
 generic_3x3:{bg:'dispenser',own:grid(62,17,3,9)},crafter_3x3:{bg:'crafter',own:grid(26,17,3,9).concat([[134,35]])},
 anvil:{bg:'anvil',own:[[27,47],[76,47],[134,47]]},beacon:{bg:'beacon',w:230,h:219,own:[[136,110]],inv:[36,137,195]},
 blast_furnace:{bg:'blast_furnace',own:[[56,17],[56,53],[116,35]]},brewing_stand:{bg:'brewing_stand',own:[[56,51],[79,58],[102,51],[79,17],[17,17]]},
 crafting:{bg:'crafting_table',own:grid(30,17,3,9).concat([[124,35]])},enchantment:{bg:'enchanting_table',own:[[15,47],[35,47]]},furnace:{bg:'furnace',own:[[56,17],[56,53],[116,35]]},
 grindstone:{bg:'grindstone',own:[[49,19],[49,40],[129,34]]},hopper:{bg:'hopper',h:133,own:grid(44,20,5,5),inv:[8,51,109]},loom:{bg:'loom',own:[[13,26],[33,26],[143,58]]},
 merchant:{bg:'villager',w:276,h:166,textureW:512,own:[[136,37],[162,37],[220,37]],inv:[108,84,142]},shulker_box:{bg:'shulker_box',own:grid(8,18,9,27)},
 smithing:{bg:'smithing',own:[[8,48],[26,48],[44,48],[98,48]]},smoker:{bg:'smoker',own:[[56,17],[56,53],[116,35]]},cartography_table:{bg:'cartography_table',own:[[15,15],[15,52],[145,39]]},stonecutter:{bg:'stonecutter',own:[[20,33],[143,33]]}
};
function renderMenu(data){const key=JSON.stringify(data||null);if(key===lastMenuKey)return;lastMenuKey=key;if(!data?.open){wrap.style.display='none';return}wrap.style.display='flex';menuEl.replaceChildren();const ownCount=Math.max(0,(data.slots||0)-36),generic=(data.type||'').startsWith('generic_9x'),rows=Math.max(1,Math.ceil(ownCount/9));let own,invX=8,invY=84,hotY=142,w=176,h=166;if(generic){const topH=17+rows*18;h=topH+96;own=grid(8,17,9,ownCount);const top=document.createElement('div');top.className='chest-top';top.style.height=topH+'px';const bottom=document.createElement('div');bottom.className='chest-bottom';bottom.style.top=topH+'px';menuEl.append(top,bottom);invY=topH+14;hotY=topH+72}else{const l=layouts[data.type]||{bg:'generic_54',own:grid(8,18,9,ownCount)};w=l.w||176;h=l.h||166;own=l.own||[];if(l.inv)[invX,invY,hotY]=l.inv;const bg=document.createElement('div');bg.className='screen-bg';bg.style.backgroundImage=`url('/assets/minecraft/textures/gui/container/${l.bg}.png?token=__TOKEN__')`;bg.style.backgroundSize=`${l.textureW||256}px 256px`;menuEl.append(bg)}menuEl.style.width=w+'px';menuEl.style.height=h+'px';const title=document.createElement('div');title.className='title';title.textContent=data.title||'';menuEl.append(title);for(let i=0;i<ownCount;i++){const p=own[i]||[8+(i%9)*18,18+Math.floor(i/9)*18];putSlot(menuEl,data.items?.[i]||null,i,p[0],p[1])}for(let i=0;i<27;i++){const index=ownCount+i;putSlot(menuEl,data.items?.[index]||null,index,invX+(i%9)*18,invY+Math.floor(i/9)*18)}for(let i=0;i<9;i++){const index=ownCount+27+i;putSlot(menuEl,data.items?.[index]||null,index,invX+i*18,hotY)}}
wrap.onclick=(e)=>{if(e.target===wrap)fetch(api('/api/close'),{method:'POST'})};
async function stateLoop(){try{const r=await fetch(api('/api/state.json'),{cache:'no-store'});if(!r.ok)throw Error(await r.text());const s=await r.json(),p=s.position;status.firstChild.textContent=p?`x ${p.x.toFixed(1)}  y ${p.y.toFixed(1)}  z ${p.z.toFixed(1)}  ·  ${s.chunks} Chunks`:'Noch nicht im Spiel';error.style.display=s.texture_error?'inline':'none';error.textContent=s.texture_error?' · '+s.texture_error:'';renderHotbar(s);renderMenu(s.menu)}catch(e){status.firstChild.textContent='Viewer getrennt';error.style.display='inline';error.textContent=' · '+e.message}setTimeout(stateLoop,document.hidden?1000:250)}
async function frameLoop(){try{const r=await fetch(api('/api/frame.png?w=426&h=240&t='+Date.now()),{cache:'no-store'});if(r.ok){const url=URL.createObjectURL(await r.blob());view.onload=()=>{if(lastUrl)URL.revokeObjectURL(lastUrl);lastUrl=url};view.onerror=()=>URL.revokeObjectURL(url);view.src=url}}catch(_){}setTimeout(frameLoop,document.hidden?1000:200)}
stateLoop();frameLoop();
</script></body></html>"###;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_liest_nur_ganze_schluessel() {
        assert_eq!(query_value("token=abc&slot=4", "slot"), Some("4"));
        assert_eq!(query_value("not_token=x", "token"), None);
    }

    #[test]
    #[cfg(feature = "menu")]
    fn unbekannte_klickart_wird_nicht_als_linksklick_ausgefuehrt() {
        assert_eq!(click_action("slot=4&action=right"), Some("rechts"));
        assert_eq!(click_action("slot=4"), Some("links"));
        assert_eq!(click_action("slot=4&action=double"), None);
    }

    #[test]
    fn zugriffstoken_hat_128_bit() {
        let token = access_token();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn seite_hat_keine_fremden_abhaengigkeiten() {
        assert!(!PAGE.contains("https://"));
        assert!(PAGE.contains("__TOKEN__"));
        assert_eq!(
            PAGE.matches("/assets/").count(),
            PAGE.matches("?token=__TOKEN__").count(),
            "jede JAR-Ressource muss den Zugriffstoken mitsenden"
        );
    }
}
