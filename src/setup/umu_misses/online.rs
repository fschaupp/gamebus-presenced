//! The `o` machinery: the store lookups and their response parsers.

use serde::Deserialize;

use super::tui::PickCandidate;
use super::{endpoints, urlencode, HTTP_TIMEOUT, USER_AGENT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GogProduct {
    pub id: String,
    pub title: String,
    pub product_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgsOffer {
    pub title: String,
    pub namespace: String,
    pub offer_type: String,
    /// The App Name of the hit's last Windows build, when the search
    /// response already carries one — Enter then commits it directly and
    /// the second request never fires.
    pub windows_app_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgsBuild {
    pub app_name: String,
    pub label_name: String,
    pub platform: String,
}

/// The `o` verb's dispatch: one lookup against the miss's effective store.
/// Exactly one HTTP request per call; failures are one honest line and
/// nothing written. Blocking — run it off the render path.
pub(crate) fn tui_online_candidates(
    store: &str,
    title: Option<&str>,
    codename: Option<&str>,
) -> Result<Vec<PickCandidate>, String> {
    match store {
        "gog" => {
            // A numeric codename IS the gogdb product id — the identity is
            // settled, only the title is in question. One exact GET on the
            // product record instead of a catalog search by a title that
            // may be the very thing that is wrong (the Spellcraft
            // incident: a shared helper exe mis-resolved the title, and a
            // search by it could never find the real game).
            if let Some(id) =
                codename.filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
            {
                let url = format!("{}/{}", endpoints().gog_product, id);
                return parse_gog_product(&http_get(&url)?).map_err(|e| format!("{url}: {e}"));
            }
            let title = require_title(title)?;
            let url = format!(
                "{}?limit=10&query=like:{}&order=desc:score&productType=in:game",
                endpoints().gog_catalog,
                urlencode(title)
            );
            parse_gog_catalog(&http_get(&url)?).map_err(|e| format!("{url}: {e}"))
        }
        "egs" => {
            let title = require_title(title)?;
            let url = format!(
                "{}?query={}&limit=10",
                endpoints().egs_search,
                urlencode(title)
            );
            parse_egs_offers(&http_get(&url)?).map_err(|e| format!("{url}: {e}"))
        }
        // The database's standalone rule pairs store none with codename
        // none — there is no store catalog to ask.
        "none" => Err(
            "Store is none — standalone entries pair codename none by the database's own rule; \
             cycle s first if this is a store game."
                .to_string(),
        ),
        other => Err(format!(
            "No online lookup for the {other} store — only gog and egs have one."
        )),
    }
}

/// The title searches have nothing to search without a title; refused
/// before any request could fire.
fn require_title(title: Option<&str>) -> Result<&str, String> {
    title
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "No resolved title to look up online.".to_string())
}

/// The second egs request: the builds of one namespace, fired by Enter on
/// an offer that carried no Windows build. One request, like every `o` step.
pub(crate) fn tui_egs_builds(namespace: &str) -> Result<Vec<PickCandidate>, String> {
    let url = format!(
        "{}/{}/builds",
        endpoints().egs_sandboxes,
        urlencode(namespace)
    );
    parse_egs_builds(&http_get(&url)?).map_err(|e| format!("{url}: {e}"))
}

/// One GET, body as text. Timeout and User-Agent like every request here.
fn http_get(url: &str) -> Result<String, String> {
    ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| format!("{url}: {e}"))?
        .into_string()
        .map_err(|e| format!("{url}: reading the response body: {e}"))
}

/// catalog.gog.com answers `{"products":[{id, title, productType, …}]}` —
/// the id is the numeric product id the database wants as the codename.
/// Rows missing an id or title are dropped, not errors.
fn parse_gog_catalog(raw: &str) -> Result<Vec<PickCandidate>, String> {
    #[derive(Deserialize)]
    struct Resp {
        products: Vec<Product>,
    }
    #[derive(Deserialize)]
    struct Product {
        id: Option<serde_json::Value>,
        title: Option<String>,
        #[serde(rename = "productType")]
        product_type: Option<String>,
    }
    let resp: Resp =
        serde_json::from_str(raw).map_err(|e| format!("not the catalog response: {e}"))?;
    Ok(resp
        .products
        .into_iter()
        .filter_map(|p| {
            // The id arrives as a JSON string; a number would mean the same.
            let id = match p.id? {
                serde_json::Value::String(s) if !s.is_empty() => s,
                serde_json::Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some(PickCandidate::GogProduct(GogProduct {
                id,
                title: p.title.filter(|t| !t.is_empty())?,
                product_type: p.product_type.unwrap_or_default(),
            }))
        })
        .collect())
}

/// api.gog.com/products/<id> answers `{"id":1660194629,"title":"Project
/// Hospital","game_type":"game","slug":"…"}` (captured live) — one record,
/// one candidate. A record without a title has nothing to offer: an error,
/// not an empty list that would read as "no such product".
fn parse_gog_product(raw: &str) -> Result<Vec<PickCandidate>, String> {
    #[derive(Deserialize)]
    struct Resp {
        id: Option<serde_json::Value>,
        title: Option<String>,
        game_type: Option<String>,
    }
    let resp: Resp = serde_json::from_str(raw).map_err(|e| format!("not a product record: {e}"))?;
    // The id arrives as a JSON number; a string would mean the same.
    let id = match resp.id {
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) if !s.is_empty() => s,
        _ => return Err("product record carries no id".into()),
    };
    let title = resp
        .title
        .filter(|t| !t.is_empty())
        .ok_or("product record carries no title")?;
    Ok(vec![PickCandidate::GogById {
        id,
        title,
        game_type: resp.game_type.unwrap_or_default(),
    }])
}

/// api.egdata.app/multisearch/offers answers `{"hits":[{title, namespace,
/// offerType, lastBuilds:[{appName, labelName, platform}], …}]}`. BASE_GAME
/// hits rank first; a hit whose lastBuilds already carries a Windows build
/// remembers its App Name so Enter can skip the second request.
fn parse_egs_offers(raw: &str) -> Result<Vec<PickCandidate>, String> {
    #[derive(Deserialize)]
    struct Resp {
        hits: Vec<Hit>,
    }
    #[derive(Deserialize)]
    struct Hit {
        title: Option<String>,
        namespace: Option<String>,
        #[serde(rename = "offerType")]
        offer_type: Option<String>,
        #[serde(rename = "lastBuilds", default)]
        last_builds: Option<Vec<RawBuild>>,
    }
    #[derive(Deserialize)]
    struct RawBuild {
        #[serde(rename = "appName")]
        app_name: Option<String>,
        platform: Option<String>,
    }
    let resp: Resp =
        serde_json::from_str(raw).map_err(|e| format!("not the offer-search response: {e}"))?;
    let mut offers: Vec<EgsOffer> = resp
        .hits
        .into_iter()
        .filter_map(|h| {
            let windows_app_name = h
                .last_builds
                .as_deref()
                .unwrap_or_default()
                .iter()
                .find_map(|b| match (&b.app_name, &b.platform) {
                    (Some(app), Some(p)) if p == "Windows" && !app.is_empty() => Some(app.clone()),
                    _ => None,
                });
            Some(EgsOffer {
                title: h.title.filter(|t| !t.is_empty())?,
                namespace: h.namespace.filter(|n| !n.is_empty())?,
                offer_type: h.offer_type.unwrap_or_default(),
                windows_app_name,
            })
        })
        .collect();
    // Stable: BASE_GAME first, the response's relevance order otherwise.
    offers.sort_by_key(|o| o.offer_type != "BASE_GAME");
    Ok(offers.into_iter().map(PickCandidate::EgsOffer).collect())
}

/// The sandboxes builds list — parsed by value, since only the rows'
/// `appName`/`labelName`/`platform` matter: a bare array and an array under
/// any top-level key both work. Live Windows builds rank first.
fn parse_egs_builds(raw: &str) -> Result<Vec<PickCandidate>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("not the builds response: {e}"))?;
    let rows = match &v {
        serde_json::Value::Array(a) => a.as_slice(),
        serde_json::Value::Object(o) => o
            .values()
            .find_map(|x| x.as_array())
            .map(Vec::as_slice)
            .ok_or("no builds array in the response")?,
        _ => return Err("no builds array in the response".into()),
    };
    let mut builds: Vec<EgsBuild> = rows
        .iter()
        .filter_map(|r| {
            let field = |k: &str| Some(r.get(k)?.as_str().unwrap_or_default().to_string());
            Some(EgsBuild {
                app_name: field("appName").filter(|a| !a.is_empty())?,
                label_name: field("labelName").unwrap_or_default(),
                platform: field("platform").unwrap_or_default(),
            })
        })
        .collect();
    builds.sort_by_key(|b| (b.platform != "Windows", !b.label_name.starts_with("Live")));
    Ok(builds.into_iter().map(PickCandidate::EgsBuild).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- The `o` dispatch: refusals happen before any request could fire.

    #[test]
    fn online_lookup_refuses_store_none_and_unknown_stores() {
        let err = tui_online_candidates("none", Some("Some Game"), None).unwrap_err();
        assert!(err.contains("cycle s first"), "{err}");
        let err = tui_online_candidates("ubisoft", Some("Some Game"), None).unwrap_err();
        assert!(err.contains("only gog and egs"), "{err}");
    }

    #[test]
    fn title_searches_refuse_without_a_title_before_any_request() {
        // A name-shaped gog codename falls to the catalog search, which has
        // nothing to search — refused before a request could fire. Same for
        // egs. (The numeric-codename by-id path needs no title at all.)
        let err = tui_online_candidates("gog", None, Some("witchery")).unwrap_err();
        assert!(err.contains("No resolved title"), "{err}");
        let err = tui_online_candidates("egs", None, None).unwrap_err();
        assert!(err.contains("No resolved title"), "{err}");
    }

    #[test]
    fn the_gog_product_record_yields_one_title_candidate() {
        // Captured live from api.gog.com/products/1660194629 — the
        // Spellcraft incident's real fix.
        let raw = r#"{"id":1660194629,"title":"Project Hospital","game_type":"game","slug":"project_hospital"}"#;
        let hits = parse_gog_product(raw).unwrap();
        assert_eq!(
            hits,
            vec![PickCandidate::GogById {
                id: "1660194629".into(),
                title: "Project Hospital".into(),
                game_type: "game".into(),
            }]
        );
        assert!(
            hits[0].section_label().contains("1660194629"),
            "{}",
            hits[0].section_label()
        );
        // A record without a title is an error, not "no such product".
        assert!(parse_gog_product(r#"{"id":1660194629}"#).is_err());
        assert!(parse_gog_product("not json").is_err());
    }

    // ---- Online response parsing (shapes from the live APIs; the requests
    // themselves stay untested — no network in the suite).

    #[test]
    fn the_gog_catalog_response_yields_product_candidates() {
        let raw = r#"{"pages":1,"products":[
            {"id":"2049187585","title":"Control Ultimate Edition","productType":"game","slug":"control"},
            {"id":1423049311,"title":"Witchery","productType":"game"},
            {"title":"No Id Ever"},
            {"id":"","title":"Empty Id"}
        ]}"#;
        let hits = parse_gog_catalog(raw).unwrap();
        assert_eq!(hits.len(), 2);
        // Both id encodings arrive as the numeric codename string.
        assert_eq!(
            hits[0],
            PickCandidate::GogProduct(GogProduct {
                id: "2049187585".into(),
                title: "Control Ultimate Edition".into(),
                product_type: "game".into(),
            })
        );
        assert!(
            matches!(&hits[1], PickCandidate::GogProduct(p) if p.id == "1423049311"),
            "{hits:?}"
        );
        assert!(parse_gog_catalog("not json").is_err());
    }

    #[test]
    fn egs_offers_rank_base_games_first_and_carry_the_windows_build() {
        let raw = r#"{"hits":[
            {"title":"Control DLC","namespace":"calluna","offerType":"DLC","lastBuilds":[]},
            {"title":"Control","namespace":"calluna","offerType":"BASE_GAME",
             "lastBuilds":[{"appName":"CallunaMac","labelName":"Live","platform":"Mac"},
                           {"appName":"Calluna","labelName":"Live","platform":"Windows"}]},
            {"title":"Mystery","namespace":"mys","offerType":"BASE_GAME"}
        ]}"#;
        let hits = parse_egs_offers(raw).unwrap();
        assert_eq!(hits.len(), 3);
        let PickCandidate::EgsOffer(first) = &hits[0] else {
            panic!("{hits:?}");
        };
        // BASE_GAME first, and the Windows build's App Name remembered —
        // the capitalized Builds App Name, never the lowercase namespace.
        assert_eq!(first.title, "Control");
        assert_eq!(first.windows_app_name.as_deref(), Some("Calluna"));
        let PickCandidate::EgsOffer(last) = &hits[2] else {
            panic!("{hits:?}");
        };
        assert_eq!(last.offer_type, "DLC");
        // No lastBuilds at all parses fine and offers the builds request.
        let PickCandidate::EgsOffer(mystery) = &hits[1] else {
            panic!("{hits:?}");
        };
        assert!(mystery.windows_app_name.is_none());
    }

    #[test]
    fn egs_builds_parse_both_shapes_and_rank_live_windows_first() {
        let bare = r#"[
            {"appName":"CallunaMac","labelName":"Live","platform":"Mac"},
            {"appName":"CallunaStaging","labelName":"Staging","platform":"Windows"},
            {"appName":"Calluna","labelName":"Live","platform":"Windows"},
            {"labelName":"Live","platform":"Windows"}
        ]"#;
        let hits = parse_egs_builds(bare).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(
            hits[0],
            PickCandidate::EgsBuild(EgsBuild {
                app_name: "Calluna".into(),
                label_name: "Live".into(),
                platform: "Windows".into(),
            })
        );
        // The same rows under a top-level key parse identically.
        let wrapped = format!("{{\"elements\":{bare}}}");
        assert_eq!(parse_egs_builds(&wrapped).unwrap(), hits);
        assert!(parse_egs_builds("{\"total\":0}").is_err());
    }
}
