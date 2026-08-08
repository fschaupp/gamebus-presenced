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
    title: &str,
) -> Result<Vec<PickCandidate>, String> {
    match store {
        "gog" => {
            let url = format!(
                "{}?limit=10&query=like:{}&order=desc:score&productType=in:game",
                endpoints().gog_catalog,
                urlencode(title)
            );
            parse_gog_catalog(&http_get(&url)?).map_err(|e| format!("{url}: {e}"))
        }
        "egs" => {
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
        let err = tui_online_candidates("none", "Some Game").unwrap_err();
        assert!(err.contains("cycle s first"), "{err}");
        let err = tui_online_candidates("ubisoft", "Some Game").unwrap_err();
        assert!(err.contains("only gog and egs"), "{err}");
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
