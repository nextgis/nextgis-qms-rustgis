//! NextGIS QuickMapServices plugin for RustGIS
use rustgis_plugin::{host, json, Kind, Node, Panel, Request, Value};
use std::collections::HashSet;

const API: &str = "https://qms.nextgis.com/api/v1/geoservices/";
const MANIFEST: &str = include_str!("../plugin.json");

fn tr(req: &Request, text: &str) -> String {
    req.tr(MANIFEST, text, &[])
}

fn tr_args(req: &Request, text: &str, args: &[(&str, &str)]) -> String {
    req.tr(MANIFEST, text, args)
}

const CONTRIBUTE_URL: &str = "https://qms.nextgis.com/create";
const CONTRIBUTE_MESSAGE: &str = "QuickMapServices is a free, crowdsourced catalogue of basemaps and geospatial services. Anyone can contribute a service and help the whole GIS community. Add yours at {url}";

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 48;

fn get_json(req: &Request, url: &str) -> Result<Option<Value>, String> {
    let answer = host::call("host.fetch", json!({"url": url}))?;
    if answer.get("pending").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    if let Some(parsed) = answer.get("json") {
        return Ok(Some(parsed.clone()));
    }
    let text = answer.get("text").and_then(Value::as_str)
        .ok_or_else(|| tr(req, "QMS API did not return JSON text"))?;
    rustgis_plugin::serde_json::from_str(text)
        .map(Some).map_err(|e| tr_args(req, "Invalid QMS response: {error}", &[("error", &e.to_string())]))
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(byte) {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn query(req: &Request) -> &str {
    req.panel.get("search-form")
        .and_then(|v| v.get("query"))
        .and_then(Value::as_str).unwrap_or("")
}

fn selected_id(req: &Request) -> Result<u64, String> {
    let selection = req.panel_str("results").ok_or_else(|| tr(req, "Select a service first"))?;
    let number = selection.strip_prefix("service:")
        .ok_or_else(|| tr(req, "Select a service, not an informational row"))?;
    number.parse::<u64>().map_err(|_| tr(req, "Invalid QMS service ID"))
}

fn supported_url(raw: &str) -> bool {
    (raw.starts_with("https://") || raw.starts_with("http://"))
        && !raw.chars().any(char::is_whitespace)
        && raw.split("//").nth(1).is_some_and(|s| !s.is_empty() && !s.starts_with('@'))
}

fn qms_detail(req: &Request, id: u64) -> Result<Option<Value>, String> {
    get_json(req, &format!("{API}{id}/"))
}

fn initial_state(req: &Request) -> Value {
    let mut response = initial_panel().answer(json!({}));
    response["panel"]["items"]["search-form"]["fields"] = json!([
        {"id": "query", "kind": "text", "label": tr(req, "Find a service"), "default": ""}
    ]);
    response
}

fn initial_panel() -> Panel {
    Panel::new().nodes("results", Vec::<Node>::new())
        .enabled("add", false).enabled("details", false)
        .visible("results", false)
        .visible("sublayer", false)
        .visible("message", false)
        .text("message", "")
}

fn search(req: &Request) -> Result<Value, String> {
    let query = query(req).trim();
    if query.is_empty() {
        return Ok(initial_panel().value("results", Value::Null).answer(json!({})));
    }
    if query.chars().count() < 3 {
        return Ok(initial_panel()
            .visible("results", true)
            .nodes("results", [Node::new("notice", tr(req, "Need at least 3 symbols to start searching..."))])
            .value("results", Value::Null)
            .answer(json!({})));
    }
    if query.chars().count() > 200 {
        return Ok(initial_panel().visible("message", true)
            .text("message", tr(req, "Search text must be at most 200 characters.")).answer(json!({})));
    }

    let mut nodes = Vec::<Node>::new();
    let mut seen = HashSet::<u64>::new();
    let mut total: Option<u64> = None;
    let mut clipped = false;

    for page in 0..MAX_PAGES {
        let offset = page * PAGE_SIZE;
        let url = format!("{API}?search={}&limit={PAGE_SIZE}&offset={offset}", percent_encode(query));
        let Some(payload) = get_json(req, &url)? else {
            return Ok(json!({})); // Fetch pending: RustGIS will repeat the command.
        };
        let (records, more) = if let Some(array) = payload.as_array() {
            (array, false)
        } else {
            total = payload.get("count").and_then(Value::as_u64).or(total);
            let records = payload.get("results").and_then(Value::as_array)
                .ok_or_else(|| tr(req, "Invalid QMS results list"))?;
            (records, payload.get("next").is_some_and(|v| {
                v.as_str().is_some_and(|s| !s.is_empty()) || v.as_bool() == Some(true)
            }))
        };
        let empty = records.is_empty();
        for record in records {
            let Some(id) = record.get("id").and_then(Value::as_u64) else { continue };
            if !seen.insert(id) { continue; }
            let name = record.get("name").and_then(Value::as_str).unwrap_or("Untitled service");
            let kind = record.get("type").and_then(Value::as_str).unwrap_or("service").to_uppercase();
            let concise: String = (if name == "Untitled service" { tr(req, name) } else { name.to_owned() })
                .chars().take(145).collect();
            nodes.push(Node::new(format!("service:{id}"), format!("{concise} · {kind}")));
        }
        if empty || !more { break; }
        if page + 1 == MAX_PAGES { clipped = true; }
    }

    let count = nodes.len();
    let message = if clipped {
        tr_args(req, "Showing {count} matches; QMS returned more than this panel can load. Refine your search.", &[("count", &count.to_string())])
    } else if count == 0 {
        tr(req, "No services found.")
    } else if let Some(all) = total {
        tr_args(req, "{count} of {all} matching services.", &[("count", &count.to_string()), ("all", &all.to_string())])
    } else {
        tr_args(req, "{count} matching services.", &[("count", &count.to_string())])
    };
    Ok(initial_panel().visible("results", true).visible("message", true)
        .nodes("results", nodes).value("results", Value::Null)
        .text("message", message).answer(json!({})))
}

fn service_type(detail: &Value) -> String {
    detail.get("type").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase().replace('-', "_")
}

fn layer_names(info: &Value) -> Vec<(String, String)> {
    let list = info.get("layers").and_then(Value::as_array).or_else(|| info.as_array());
    list.into_iter().flatten().filter_map(|layer| {
        let key = layer.get("name").or_else(|| layer.get("layer")).or_else(|| layer.get("id"))?;
        let id = key.as_str().map(str::to_string).unwrap_or_else(|| key.to_string());
        let title = layer.get("title").and_then(Value::as_str).unwrap_or(&id).to_owned();
        Some((id, title))
    }).collect()
}

fn select(req: &Request) -> Result<Value, String> {
    let Ok(id) = selected_id(req) else {
        return Ok(Panel::new().enabled("add", false).enabled("details", false)
            .visible("sublayer", false).value("sublayer", Value::Null)
            .visible("message", false).answer(json!({})));
    };
    let Some(detail) = qms_detail(req, id)? else { return Ok(json!({})); };
    let kind = service_type(&detail);
    let mut panel = Panel::new().enabled("add", true).enabled("details", true)
        .visible("sublayer", false).options("sublayer", Vec::<Node>::new())
        .value("sublayer", Value::Null)
        .visible("message", false);

    if kind == "wms" || kind == "wfs" {
        if let Some(url) = detail.get("url").and_then(Value::as_str).filter(|s| supported_url(s)) {
            match host::call("service.layers", json!({"url":url,"type":kind})) {
                Ok(answer) if answer.get("pending").and_then(Value::as_bool) == Some(true) => {
                    return Ok(json!({})); // Native host can re-run when cached.
                }
                Ok(answer) => {
                    let layers = layer_names(&answer);
                    if layers.len() > 1 {
                        panel = panel.visible("sublayer", true)
                            .options("sublayer", layers.iter().take(1000).map(|(key, title)| Node::new(key, title)));
                        panel = panel.visible("message", true)
                            .text("message", tr(req, "Choose a WMS/WFS sublayer, then click Add service."));
                    } else if layers.is_empty() {
                        panel = panel.enabled("add", false).visible("message", true)
                            .text("message", tr(req, "RustGIS could not discover a supported WMS/WFS sublayer."));
                    }
                }
                Err(e) => {
                    panel = panel.enabled("add", false).visible("message", true)
                        .text("message", tr_args(req, "Cannot list sublayers: {error}", &[("error", &e)]));
                }
            }
        } else {
            panel = panel.enabled("add", false).visible("message", true)
                .text("message", tr(req, "This QMS service has no valid WMS/WFS address."));
        }
    }
    Ok(panel.answer(json!({})))
}

fn add(req: &Request) -> Result<Value, String> {
    let id = selected_id(req)?;
    let Some(detail) = qms_detail(req, id)? else { return Ok(json!({})); };
    let name_owned = detail.get("name").and_then(Value::as_str)
        .map(str::to_owned).unwrap_or_else(|| tr(req, "QMS service"));
    let name = name_owned.as_str();
    let kind = service_type(&detail);
    let raw = detail.get("url").and_then(Value::as_str).unwrap_or("");
    if !supported_url(raw) { return Err(tr(req, "This QMS service has no valid HTTP(S) URL.")); }

    let outcome = match kind.as_str() {
        "tms" | "xyz" => {
            let url = if detail.get("y_origin_top").and_then(Value::as_bool) == Some(false) {
                raw.replace("{y}", "{-y}")
            } else { raw.to_string() };
            let mut opts = json!({"url":url,"name":name,"overlay":true,"replace":false});
            let credit = detail.get("copyright_text").and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .or_else(|| detail.get("license_name").and_then(Value::as_str));
            if let Some(credit) = credit { opts["attribution"] = json!(credit); }
            if let Some(zmax) = detail.get("z_max").and_then(Value::as_u64) {
                opts["maxZoom"] = json!(zmax.min(30));
            }
            host::call("map.addTiles", opts)?
        }
        "mvt" | "vector" | "vector_tile" | "vector_tiles" | "vectortile" | "vectortiles" => {
            host::call("map.addTiles", json!({"url":raw,"name":name,"overlay":true,"replace":false}))?
        }
        "geojson" => host::call("layer.addUrl", json!({"url":raw,"name":name}))?,
        "wms" | "wfs" => {
            let info = host::call("service.layers", json!({"url":raw,"type":kind}))?;
            if info.get("pending").and_then(Value::as_bool) == Some(true) {
                return Ok(json!({}));
            }
            let layers = layer_names(&info);
            if layers.is_empty() { return Err(tr_args(req, "No supported {type} sublayers were discovered.", &[("type", &kind.to_uppercase())])); }
            let picked = req.panel_str("sublayer");
            let chosen = if layers.len() == 1 {
                &layers[0].0
            } else {
                let choice = picked.ok_or_else(|| tr(req, "Choose a WMS/WFS sublayer before adding this service."))?;
                &layers.iter().find(|(id, _)| id == choice)
                    .ok_or_else(|| tr(req, "The selected sublayer is unavailable; select the service again."))?.0
            };
            host::call("service.add", json!({"url":raw,"type":kind,"layer":chosen}))?
        }
        _ => return Err(tr_args(req, "Unsupported QMS service type: {type}", &[("type", &kind)])),
    };
    if outcome.get("pending").and_then(Value::as_bool) == Some(true) {
        return Ok(json!({})); // The SDK host completes this addition and re-runs the command.
    }
    host::notify(&tr_args(req, "Added QMS service: {name}", &[("name", name)]));
    Ok(Panel::new().visible("message", true).text("message", tr_args(req, "Added: {name}", &[("name", name)]))
        .answer(json!({"service":id,"added":true})))
}

fn details(req: &Request) -> Result<Value, String> {
    let id = selected_id(req)?;
    let Some(detail) = qms_detail(req, id)? else { return Ok(json!({})); };
    let name_owned = detail.get("name").and_then(Value::as_str)
        .map(str::to_owned).unwrap_or_else(|| tr(req, "QMS service"));
    let name = name_owned.as_str();
    let desc = detail.get("desc").and_then(Value::as_str).unwrap_or("");
    let link = format!("https://qms.nextgis.com/geoservices/{id}/");
    let message = format!("{name}\n{desc}\n{}", tr_args(req, "Service information: {url}", &[("url", &link)]));
    Ok(Panel::new().visible("message", true).text("message", message.clone()).answer(json!({"report":message})))
}

fn contribute(req: &Request) -> Result<Value, String> {
    let message = tr_args(req, CONTRIBUTE_MESSAGE, &[("url", CONTRIBUTE_URL)]);
    host::notify(&message);
    Ok(Panel::new()
        .visible("message", true)
        .text("message", &message)
        .answer(json!({"report": message})))
}

fn handle(req: Request) -> Result<Value, String> {
    if req.kind != Kind::Command { return Err(tr(&req, "QMS has no geoprocessing tools")); }
    match req.id.as_str() {
        "init" => Ok(initial_state(&req)),
        "search" => search(&req),
        "select" => select(&req),
        "add" => add(&req),
        "details" => details(&req),
        "contribute" => contribute(&req),
        other => Err(tr_args(&req, "Unknown QMS action: {action}", &[("action", other)])),
    }
}

rustgis_plugin::export!(handle);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_url_encoding() {
        assert_eq!(percent_encode("New York & море"), "New%20York%20%26%20%D0%BC%D0%BE%D1%80%D0%B5");
    }
    #[test]
    fn service_node_id_is_numeric() {
        let node = Node::new("service:149", "OSM");
        assert_eq!(node.id, "service:149");
    }
    #[test]
    fn reject_invalid_scheme() {
        assert!(!supported_url("file:///etc/passwd"));
        assert!(supported_url("https://example.org/{z}/{x}/{y}.png"));
    }
    #[test]
    fn derive_layer_keys_from_native_service_list() {
        let sample = json!({"layers":[{"name":"elevation", "title":"Elevation"}, {"name":"roads", "title":"Roads"}]});
        assert_eq!(layer_names(&sample).len(), 2);
        assert_eq!(layer_names(&sample)[1].0, "roads");
    }
    #[test]
    fn contribution_message_explains_how_and_why() {
        assert!(CONTRIBUTE_MESSAGE.contains("free"));
        assert!(CONTRIBUTE_MESSAGE.contains("crowdsourced"));
        assert!(CONTRIBUTE_MESSAGE.contains("community"));
        assert_eq!(CONTRIBUTE_URL, "https://qms.nextgis.com/create");
    }

    fn test_request(lang: &str) -> Request {
        Request {
            kind: Kind::Command, id: "init".into(), params: Value::Null,
            layers: Default::default(), files: Default::default(),
            lang: lang.into(), panel: Default::default(), panel_changed: Vec::new(),
        }
    }

    #[test]
    fn manifest_translates_live_panel_text_and_placeholders() {
        let ru = test_request("ru");
        assert_eq!(tr(&ru, "Find a service"), "Найти сервис");
        let message = tr_args(&ru, "Added: {name}", &[("name", "OpenStreetMap")]);
        assert!(message.contains("OpenStreetMap"));
        assert!(!message.contains("{name}"));
        let zh_tw = test_request("zh-tw");
        assert_ne!(tr(&zh_tw, "Find a service"), tr(&test_request("zh"), "Find a service"));
        assert_ne!(tr(&test_request("bn"), "Find a service"), "Find a service");
        assert_eq!(tr(&test_request("xx"), "Find a service"), "Find a service");
    }

    #[test]
    fn initialized_form_has_a_text_field() {
        let state = initial_state(&test_request("en"));
        let fields = state["panel"]["items"]["search-form"]["fields"]
            .as_array().expect("search form fields must be supplied in the state response");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0]["id"], "query");
        assert_eq!(fields[0]["kind"], "text");
        assert_eq!(fields[0]["label"], "Find a service");
        assert_eq!(state["panel"]["items"]["results"]["visible"], false);
    }

}
