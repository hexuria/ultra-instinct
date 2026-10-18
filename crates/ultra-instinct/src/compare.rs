//! Fixture id and action agreement.
//!
//! Four local fixtures are ranked with `WeightedMatcher`. The press fixture is
//! the only one handed to `BrowserExecutor`. A manifold file cannot act.
//! [`eval_corpus`] ranks the locate corpus with both matchers and names no winner.
//! This is not a Browser Use score and it is not a `ComputerResult`: that type
//! always sets `executed` and `verified`, so a locate would look like a fake
//! refusal.
//!
//! System One compiles only with the `jev` feature and runs only when
//! `ULTRA_INSTINCT_JEV=1`. `agree` is choice equality. It is not a win.

use std::fs;
use std::path::Path;

use ultra_instinct_browser::{BrowserSession, ReplayTransport};
use ultra_instinct_core::{parse_fixture, InteractionManifold, LocateQuery, Role, Zone};
use ultra_instinct_guard::{margin_millis, MIN_ALLOW_CONFIDENCE};
use ultra_instinct_resonance::{Match, RegionMatcher, WeightedMatcher};

#[cfg(any(test, feature = "jev"))]
use ultra_instinct_core::InteractionRegion;
#[cfg(feature = "jev")]
use ultra_instinct_remote::typesafe_sdk;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompareError {
    Io {
        path: String,
        message: String,
    },
    Fixture(String),
    Locate(String),
    EmptyRank {
        fixture: String,
    },
    UnknownRegion(String),
    /// A `.manifold` file has no CDP script. Acting would pretend it could.
    ManifoldCannotAct {
        fixture: String,
    },
    Browser(String),
    NonFiniteConfidence,
    /// The executor reported a low score and still logged a CDP call.
    TransportCalledBelowThreshold,
    Corpus {
        path: String,
        message: String,
    },
    #[cfg(feature = "jev")]
    Jev(String),
    #[cfg(feature = "jev")]
    ChoiceNotOffered {
        choice: String,
    },
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "cannot read {path}: {message}"),
            Self::Fixture(message) => write!(f, "fixture: {message}"),
            Self::Locate(message) => write!(f, "locate: {message}"),
            Self::EmptyRank { fixture } => write!(f, "{fixture} has no regions to rank"),
            Self::UnknownRegion(id) => write!(f, "unknown region `{id}`"),
            Self::ManifoldCannotAct { fixture } => {
                write!(f, "{fixture} is a manifold fixture and cannot act")
            }
            Self::Browser(message) => f.write_str(message),
            Self::NonFiniteConfidence => f.write_str("confidence must be finite"),
            Self::TransportCalledBelowThreshold => {
                f.write_str("scored confidence is below 0.55 but the transport was called")
            }
            Self::Corpus { path, message } => write!(f, "eval corpus {path}: {message}"),
            #[cfg(feature = "jev")]
            Self::Jev(message) => write!(f, "system one: {message}"),
            #[cfg(feature = "jev")]
            Self::ChoiceNotOffered { choice } => {
                write!(
                    f,
                    "system one choice `{choice}` was not one of the offered options"
                )
            }
        }
    }
}

impl std::error::Error for CompareError {}

#[derive(Clone, Copy)]
struct Spec {
    file: &'static str,
    text: &'static str,
    role: Option<Role>,
    position: Option<Zone>,
    act: bool,
}

/// `press-only.cdp.json` is omitted: it has no observation. Sign-in is not
/// diffed against welcome.
const SPECS: [Spec; 4] = [
    Spec {
        file: "sidebar.manifold",
        text: "Settings",
        role: Some(Role::Button),
        position: Some(Zone::Left),
        act: false,
    },
    Spec {
        file: "sign-in.cdp.json",
        text: "Sign in",
        role: None,
        position: None,
        act: false,
    },
    Spec {
        file: "welcome.cdp.json",
        text: "Welcome",
        role: None,
        position: None,
        act: false,
    },
    Spec {
        file: "sign-in-press.cdp.json",
        text: "Sign in",
        role: None,
        position: None,
        act: true,
    },
];

/// One measured case. Absent fields were not measured and are omitted from JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct FixtureCase {
    fixture: String,
    region_id: String,
    confidence: f64,
    executed: Option<bool>,
    jev_choice: Option<String>,
    agree: Option<bool>,
}

impl FixtureCase {
    pub fn fixture(&self) -> &str {
        &self.fixture
    }

    pub fn region_id(&self) -> &str {
        &self.region_id
    }

    pub fn confidence(&self) -> f64 {
        self.confidence
    }

    /// `Some` only after an action was considered. Locate cases stay `None`.
    pub fn executed(&self) -> Option<bool> {
        self.executed
    }

    pub fn jev_choice(&self) -> Option<&str> {
        self.jev_choice.as_deref()
    }

    pub fn agree(&self) -> Option<bool> {
        self.agree
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FixtureReport {
    cases: Vec<FixtureCase>,
}

impl FixtureReport {
    pub fn cases(&self) -> &[FixtureCase] {
        &self.cases
    }

    /// One JSON document. Unmeasured keys are omitted. No nulls.
    pub fn render(&self) -> String {
        let mut out = String::from("{\n  \"cases\": [\n");
        for (index, case) in self.cases.iter().enumerate() {
            let comma = if index + 1 == self.cases.len() {
                ""
            } else {
                ","
            };
            out.push_str("    {\n");
            out.push_str(&format!(
                "      \"fixture\": \"{}\",\n",
                json_escape(&case.fixture)
            ));
            out.push_str(&format!(
                "      \"region_id\": \"{}\",\n",
                json_escape(&case.region_id)
            ));
            out.push_str("      \"confidence\": ");
            push_finite(&mut out, case.confidence);
            if let Some(executed) = case.executed {
                out.push_str(",\n      \"executed\": ");
                out.push_str(if executed { "true" } else { "false" });
            }
            if let Some(choice) = &case.jev_choice {
                out.push_str(",\n      \"jev\": { \"choice\": \"");
                out.push_str(&json_escape(choice));
                out.push_str("\" }");
            }
            if let Some(agree) = case.agree {
                out.push_str(",\n      \"agree\": ");
                out.push_str(if agree { "true" } else { "false" });
            }
            out.push('\n');
            out.push_str(&format!("    }}{comma}\n"));
        }
        out.push_str("  ]\n}\n");
        out
    }
}

/// Rank the four fixtures. No network.
pub fn fixture_compare(fixtures_dir: &Path) -> Result<FixtureReport, CompareError> {
    let mut cases = Vec::with_capacity(SPECS.len());
    for spec in SPECS {
        let body = read_fixture(fixtures_dir, spec.file)?;
        cases.push(build_case(spec, &body)?);
    }
    Ok(FixtureReport { cases })
}

/// Fixture comparison, then one System One choice per case when `ULTRA_INSTINCT_JEV=1`.
///
/// Without that variable this is [`fixture_compare`] and does not open a socket.
/// The choice must be one of the options that were sent. `agree` may be false.
#[cfg(feature = "jev")]
pub fn fixture_compare_live(fixtures_dir: &Path) -> Result<FixtureReport, CompareError> {
    let mut report = fixture_compare(fixtures_dir)?;
    if !jev_requested() {
        return Ok(report);
    }
    let client = typesafe_sdk::blocking::Client::from_env()
        .map_err(|err| CompareError::Jev(err.to_string()))?;
    for case in &mut report.cases {
        let body = read_fixture(fixtures_dir, &case.fixture)?;
        let manifold = load_manifold(&body)?;
        let offered = offered_options(&manifold, case);
        let choice = ask_choice(&client, &manifold, case)?;
        if !offered.iter().any(|option| option == &choice) {
            return Err(CompareError::ChoiceNotOffered { choice });
        }
        case.agree = Some(agrees(case, &choice));
        case.jev_choice = Some(choice);
    }
    Ok(report)
}

fn build_case(spec: Spec, body: &str) -> Result<FixtureCase, CompareError> {
    let manifold = load_manifold(body)?;
    let query = locate_query(spec)?;
    let (top, runner_up) = top_match(spec.file, &query, &manifold)?;
    if !top.confidence().is_finite() {
        return Err(CompareError::NonFiniteConfidence);
    }
    let executed = if spec.act {
        if !body.trim_start().starts_with('{') {
            return Err(CompareError::ManifoldCannotAct {
                fixture: spec.file.to_owned(),
            });
        }
        Some(scored_press(
            body,
            top.id().as_str(),
            top.confidence(),
            runner_up,
        )?)
    } else {
        None
    };
    Ok(FixtureCase {
        fixture: spec.file.to_owned(),
        region_id: top.id().as_str().to_owned(),
        confidence: top.confidence(),
        executed,
        jev_choice: None,
        agree: None,
    })
}

fn locate_query(spec: Spec) -> Result<LocateQuery, CompareError> {
    let mut query = LocateQuery::new()
        .text(spec.text)
        .map_err(|_| CompareError::Locate("empty text".to_owned()))?;
    if let Some(role) = spec.role {
        query = query.role(role);
    }
    if let Some(position) = spec.position {
        query = query.position(position);
    }
    Ok(query)
}

/// The top match and the runner-up's total, if there is one.
fn top_match(
    fixture: &str,
    query: &LocateQuery,
    manifold: &InteractionManifold,
) -> Result<(Match, Option<f64>), CompareError> {
    let ranked = WeightedMatcher::default()
        .rank(query, manifold)
        .map_err(|err| CompareError::Locate(err.to_string()))?;
    let runner_up = ranked.get(1).map(Match::confidence);
    let top = ranked
        .into_iter()
        .next()
        .ok_or_else(|| CompareError::EmptyRank {
            fixture: fixture.to_owned(),
        })?;
    Ok((top, runner_up))
}

/// Whether the firewall gate would allow a click at this confidence.
/// Ultra-Instinct no longer presses; this is the allow/refuse decision only.
pub(crate) fn scored_press(
    _script: &str,
    _region_id: &str,
    confidence: f64,
    runner_up: Option<f64>,
) -> Result<bool, CompareError> {
    if !confidence.is_finite() || runner_up.is_some_and(|v| !v.is_finite()) {
        return Err(CompareError::NonFiniteConfidence);
    }
    let (_, refuse) = gate_report(confidence, runner_up);
    Ok(!refuse)
}

fn read_fixture(dir: &Path, name: &str) -> Result<String, CompareError> {
    let path = dir.join(name);
    fs::read_to_string(&path).map_err(|err| CompareError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })
}

fn load_manifold(body: &str) -> Result<InteractionManifold, CompareError> {
    if body.trim_start().starts_with('{') {
        let transport =
            ReplayTransport::parse(body).map_err(|err| CompareError::Browser(err.to_string()))?;
        let mut session = BrowserSession::new(transport);
        session
            .observe()
            .cloned()
            .map_err(|err| CompareError::Browser(err.to_string()))
    } else {
        parse_fixture(body).map_err(|err| CompareError::Fixture(err.to_string()))
    }
}

#[cfg(any(test, feature = "jev"))]
fn agrees(case: &FixtureCase, choice: &str) -> bool {
    match case.executed {
        None => choice == case.region_id,
        Some(executed) => agrees_press(choice, executed),
    }
}

#[cfg(any(test, feature = "jev"))]
fn agrees_press(choice: &str, executed: bool) -> bool {
    match choice {
        "press" => executed,
        "do-not-press" => !executed,
        _ => false,
    }
}

#[cfg(any(test, feature = "jev"))]
fn region_prompt(label: &str) -> String {
    format!("Which region id matches the {label} control?")
}

#[cfg(any(test, feature = "jev"))]
fn press_prompt(region: &InteractionRegion, confidence: f64) -> String {
    let rect = region.rect();
    format!(
        "Selected region {id} role {role} label {label} rect x={x:?} y={y:?} w={w:?} h={h:?}. Locate confidence {confidence:?}. Choose press or do-not-press.",
        id = region.id().as_str(),
        role = region.role().as_str(),
        label = region.label(),
        x = rect.x(),
        y = rect.y(),
        w = rect.width(),
        h = rect.height(),
    )
}

#[cfg(any(test, feature = "jev"))]
fn describe_region(region: &InteractionRegion) -> String {
    let rect = region.rect();
    format!(
        "{role} {label} x={x:?} y={y:?} w={w:?} h={h:?}",
        role = region.role().as_str(),
        label = region.label(),
        x = rect.x(),
        y = rect.y(),
        w = rect.width(),
        h = rect.height(),
    )
}

fn json_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                out.push_str(&format!("\\u{:04x}", u32::from(ch)));
            }
            ch => out.push(ch),
        }
    }
    out
}

fn push_finite(out: &mut String, value: f64) {
    // Debug is a round-trip and, for a finite value, a JSON number.
    out.push_str(&format!("{value:?}"));
}

#[cfg(feature = "jev")]
fn jev_requested() -> bool {
    std::env::var("ULTRA_INSTINCT_JEV")
        .ok()
        .is_some_and(|value| value.trim() == "1")
}

#[cfg(feature = "jev")]
fn offered_options(manifold: &InteractionManifold, case: &FixtureCase) -> Vec<String> {
    if case.executed.is_some() {
        vec!["press".to_owned(), "do-not-press".to_owned()]
    } else {
        manifold
            .regions()
            .map(|region| region.id().as_str().to_owned())
            .collect()
    }
}

#[cfg(feature = "jev")]
fn ask_choice(
    client: &typesafe_sdk::blocking::Client,
    manifold: &InteractionManifold,
    case: &FixtureCase,
) -> Result<String, CompareError> {
    let state = manifold_state(manifold);
    let (name, question) = if case.executed.is_some() {
        let region = manifold
            .get_str(&case.region_id)
            .ok_or_else(|| CompareError::UnknownRegion(case.region_id.clone()))?;
        let prompt = press_prompt(region, case.confidence);
        let blank: Option<typesafe_sdk::JsonContent> = None;
        let question = typesafe_sdk::Question::choice(
            prompt,
            [("press", blank.clone()), ("do-not-press", blank)],
        );
        ("action", question)
    } else {
        let prompt = region_prompt(control_label(case)?);
        let options = manifold.regions().map(|region| {
            (
                region.id().as_str().to_owned(),
                Some(typesafe_sdk::JsonContent::from(describe_region(region))),
            )
        });
        ("region", typesafe_sdk::Question::choice(prompt, options))
    };
    let response = client
        .system_one(state, [(name, question)])
        .map_err(|err| CompareError::Jev(err.to_string()))?;
    // Choice only. Usage, probabilities, and the answer confidence are not read.
    let answered = response
        .choice(name)
        .map_err(|err| CompareError::Jev(err.to_string()))?;
    Ok(answered.choice.clone())
}

#[cfg(feature = "jev")]
fn control_label(case: &FixtureCase) -> Result<&'static str, CompareError> {
    SPECS
        .iter()
        .find(|spec| spec.file == case.fixture)
        .map(|spec| spec.text)
        .ok_or_else(|| CompareError::Locate(format!("no control label for {}", case.fixture)))
}

#[cfg(feature = "jev")]
fn manifold_state(manifold: &InteractionManifold) -> serde_json::Value {
    let viewport = manifold.viewport();
    let regions: Vec<serde_json::Value> = manifold
        .regions()
        .map(|region| {
            let rect = region.rect();
            serde_json::json!({
                "id": region.id().as_str(),
                "role": region.role().as_str(),
                "label": region.label(),
                "rect": {
                    "x": rect.x(),
                    "y": rect.y(),
                    "w": rect.width(),
                    "h": rect.height(),
                },
            })
        })
        .collect();
    serde_json::json!({
        "viewport": {
            "x": viewport.x(),
            "y": viewport.y(),
            "w": viewport.width(),
            "h": viewport.height(),
        },
        "regions": regions,
    })
}

/// One matcher's top hit. Totals are not comparable across matchers.
#[derive(Clone, Debug, PartialEq)]
pub struct CorpusHit {
    matcher: &'static str,
    top_id: String,
    margin_millis: Option<i32>,
    gate_would_refuse: bool,
}

impl CorpusHit {
    pub fn matcher(&self) -> &'static str {
        self.matcher
    }
    pub fn top_id(&self) -> &str {
        &self.top_id
    }
    pub fn margin_millis(&self) -> Option<i32> {
        self.margin_millis
    }
    pub fn gate_would_refuse(&self) -> bool {
        self.gate_would_refuse
    }
}

/// One corpus row. `hits` is weighted, then hgra. There is no winner.
#[derive(Clone, Debug, PartialEq)]
pub struct CorpusCase {
    fixture: String,
    text: String,
    role: String,
    position: String,
    expected_id: String,
    hits: Vec<CorpusHit>,
}

impl CorpusCase {
    pub fn fixture(&self) -> &str {
        &self.fixture
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn role(&self) -> &str {
        &self.role
    }
    pub fn position(&self) -> &str {
        &self.position
    }
    pub fn expected_id(&self) -> &str {
        &self.expected_id
    }
    pub fn hits(&self) -> &[CorpusHit] {
        &self.hits
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CorpusReport {
    cases: Vec<CorpusCase>,
}

impl CorpusReport {
    pub fn cases(&self) -> &[CorpusCase] {
        &self.cases
    }

    /// Both matchers, top id, margin, and whether the act gate would refuse.
    /// No winner field.
    pub fn render(&self) -> String {
        let mut out = String::from("{\n  \"cases\": [\n");
        for (index, case) in self.cases.iter().enumerate() {
            let comma = if index + 1 == self.cases.len() {
                ""
            } else {
                ","
            };
            out.push_str("    {\n");
            out.push_str(&format!(
                "      \"fixture\": \"{}\",\n",
                json_escape(&case.fixture)
            ));
            out.push_str(&format!(
                "      \"text\": \"{}\",\n",
                json_escape(&case.text)
            ));
            out.push_str(&format!(
                "      \"role\": \"{}\",\n",
                json_escape(&case.role)
            ));
            out.push_str(&format!(
                "      \"position\": \"{}\",\n",
                json_escape(&case.position)
            ));
            out.push_str(&format!(
                "      \"expected_id\": \"{}\",\n",
                json_escape(&case.expected_id)
            ));
            out.push_str("      \"hits\": [\n");
            for (hit_index, hit) in case.hits.iter().enumerate() {
                let hit_comma = if hit_index + 1 == case.hits.len() {
                    ""
                } else {
                    ","
                };
                out.push_str("        {\n");
                out.push_str(&format!("          \"matcher\": \"{}\",\n", hit.matcher));
                out.push_str(&format!(
                    "          \"top_id\": \"{}\",\n",
                    json_escape(&hit.top_id)
                ));
                out.push_str("          \"margin_millis\": ");
                match hit.margin_millis {
                    Some(margin) => out.push_str(&margin.to_string()),
                    None => out.push_str("null"),
                }
                out.push_str(",\n          \"gate_would_refuse\": ");
                out.push_str(if hit.gate_would_refuse {
                    "true"
                } else {
                    "false"
                });
                out.push('\n');
                out.push_str(&format!("        }}{hit_comma}\n"));
            }
            out.push_str("      ]\n");
            out.push_str(&format!("    }}{comma}\n"));
        }
        out.push_str("  ]\n}\n");
        out
    }
}

/// Rank every row in `dir/cases.tsv` with the weighted matcher and with HGRA.
///
/// The product default stays [`WeightedMatcher`]. This report does not compare
/// the two totals and does not name a winner. `expected_id` is checked by the
/// corpus test against the weighted top, not against HGRA.
pub fn eval_corpus(dir: &Path) -> Result<CorpusReport, CompareError> {
    let path = dir.join("cases.tsv");
    let body = fs::read_to_string(&path).map_err(|err| CompareError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    let rows = parse_cases(&path, &body)?;
    let mut cases = Vec::with_capacity(rows.len());
    for row in rows {
        let fixture_body = read_fixture(dir, &row.fixture)?;
        let manifold = load_manifold(&fixture_body)?;
        let query = corpus_query(&row)?;
        let hits = vec![
            corpus_hit(
                "weighted",
                &WeightedMatcher::default(),
                &query,
                &manifold,
                &row,
            )?,
            // HGRA corpus row omitted: experiment is feature-gated off the product path.
        ];
        cases.push(CorpusCase {
            fixture: row.fixture,
            text: row.text,
            role: row.role,
            position: row.position,
            expected_id: row.expected_id,
            hits,
        });
    }
    Ok(CorpusReport { cases })
}

struct CorpusRow {
    fixture: String,
    text: String,
    role: String,
    position: String,
    expected_id: String,
}

fn parse_cases(path: &Path, body: &str) -> Result<Vec<CorpusRow>, CompareError> {
    let mut lines = body.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next().ok_or_else(|| CompareError::Corpus {
        path: path.display().to_string(),
        message: "missing header".into(),
    })?;
    if header.split('\t').collect::<Vec<_>>()
        != ["fixture", "text", "role", "position", "expected_id"]
    {
        return Err(CompareError::Corpus {
            path: path.display().to_string(),
            message: "header must be fixture, text, role, position, expected_id".into(),
        });
    }
    let mut rows = Vec::new();
    for (index, line) in lines.enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 5 {
            return Err(CompareError::Corpus {
                path: path.display().to_string(),
                message: format!("row {} does not have 5 columns", index + 2),
            });
        }
        rows.push(CorpusRow {
            fixture: fields[0].to_owned(),
            text: fields[1].to_owned(),
            role: fields[2].to_owned(),
            position: fields[3].to_owned(),
            expected_id: fields[4].to_owned(),
        });
    }
    if rows.is_empty() {
        return Err(CompareError::Corpus {
            path: path.display().to_string(),
            message: "no cases".into(),
        });
    }
    Ok(rows)
}

fn corpus_query(row: &CorpusRow) -> Result<LocateQuery, CompareError> {
    let mut query = LocateQuery::new()
        .text(&row.text)
        .map_err(|_| CompareError::Locate("empty text".to_owned()))?;
    if !row.role.is_empty() {
        let role = Role::parse(&row.role).ok_or_else(|| CompareError::Corpus {
            path: row.fixture.clone(),
            message: format!("unknown role {}", row.role),
        })?;
        query = query.role(role);
    }
    if !row.position.is_empty() {
        let position = Zone::parse(&row.position).ok_or_else(|| CompareError::Corpus {
            path: row.fixture.clone(),
            message: format!("unknown position {}", row.position),
        })?;
        query = query.position(position);
    }
    Ok(query)
}

fn corpus_hit(
    matcher: &'static str,
    ranker: &impl RegionMatcher,
    query: &LocateQuery,
    manifold: &InteractionManifold,
    row: &CorpusRow,
) -> Result<CorpusHit, CompareError> {
    let ranked = ranker
        .rank(query, manifold)
        .map_err(|err| CompareError::Locate(err.to_string()))?;
    let top = ranked.first().ok_or_else(|| CompareError::EmptyRank {
        fixture: row.fixture.clone(),
    })?;
    let runner_up = ranked.get(1).map(Match::confidence);
    let (margin_millis, gate_would_refuse) = gate_report(top.confidence(), runner_up);
    Ok(CorpusHit {
        matcher,
        top_id: top.id().as_str().to_owned(),
        margin_millis,
        gate_would_refuse,
    })
}

/// The product allow gate. A missing runner-up checks only the 0.55 threshold.
fn gate_report(top: f64, runner_up: Option<f64>) -> (Option<i32>, bool) {
    use ultra_instinct_guard::{MARGIN_EPSILON, MIN_ALLOW_MARGIN};
    let margin = runner_up.map(|second| margin_millis(top, second));
    let refused = if top < MIN_ALLOW_CONFIDENCE {
        true
    } else if let Some(second) = runner_up {
        let gap = top - second;
        !gap.is_finite() || gap < MIN_ALLOW_MARGIN - MARGIN_EPSILON
    } else {
        false
    };
    (margin, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn query(text: &str, role: Option<Role>, position: Option<Zone>) -> LocateQuery {
        let mut query = LocateQuery::new().text(text).unwrap();
        if let Some(role) = role {
            query = query.role(role);
        }
        if let Some(position) = position {
            query = query.position(position);
        }
        query
    }

    #[test]
    fn fixture_report_matches_region_ids_and_a_second_rank() {
        let dir = fixtures_dir();
        let report = fixture_compare(&dir).unwrap();
        let expected = [
            (
                "sidebar.manifold",
                "nav-settings",
                query("Settings", Some(Role::Button), Some(Zone::Left)),
                false,
            ),
            (
                "sign-in.cdp.json",
                "n100",
                query("Sign in", None, None),
                false,
            ),
            (
                "welcome.cdp.json",
                "n300",
                query("Welcome", None, None),
                false,
            ),
            (
                "sign-in-press.cdp.json",
                "n100",
                query("Sign in", None, None),
                true,
            ),
        ];
        assert_eq!(report.cases().len(), expected.len());
        for (case, (file, region_id, locate, acted)) in report.cases().iter().zip(expected) {
            assert_eq!(case.fixture(), file);
            assert_eq!(case.region_id(), region_id);
            assert_eq!(case.executed().is_some(), acted);
            assert!(case.jev_choice().is_none());
            assert!(case.agree().is_none());
            let body = fs::read_to_string(dir.join(file)).unwrap();
            let manifold = load_manifold(&body).unwrap();
            let again = WeightedMatcher::default().rank(&locate, &manifold).unwrap();
            assert_eq!(case.confidence(), again[0].confidence());
            assert_eq!(again[0].id().as_str(), region_id);
        }
        let press = report
            .cases()
            .iter()
            .find(|case| case.fixture() == "sign-in-press.cdp.json")
            .unwrap();
        if press.confidence() >= MIN_ALLOW_CONFIDENCE {
            assert_eq!(press.executed(), Some(true));
        } else {
            assert_eq!(press.executed(), Some(false));
        }
        let json = report.render();
        assert_eq!(json.matches("\"executed\"").count(), 1);
        assert!(!json.contains("\"jev\""));
        assert!(!json.contains("\"agree\""));
        for forbidden in [
            "browser_use",
            "tokens",
            "screenshot",
            "retries",
            "latency_ms",
            "mechanism",
            "verified",
            "benchmark",
            "winner",
            "fallback",
            "press-only",
            "null",
        ] {
            assert!(!json.contains(forbidden), "{forbidden} in {json}");
        }
    }

    #[test]
    fn below_threshold_does_not_execute_and_does_not_call_transport() {
        let script = include_str!("../../../fixtures/sign-in-press.cdp.json");
        let executed = scored_press(script, "n100", 0.49, None).unwrap();
        assert!(!executed);
    }

    #[test]
    fn below_margin_does_not_execute_and_does_not_call_transport() {
        let script = include_str!("../../../fixtures/sign-in-press.cdp.json");
        assert!(!scored_press(script, "n100", 1.0, Some(0.98)).unwrap());
        assert!(scored_press(script, "n100", 1.0, Some(0.5)).unwrap());
    }

    #[ignore = "firewall pivot; re-home under guard"]
    #[test]
    fn transport_failure_is_not_recorded_as_executed() {
        let err = scored_press(r#"{"calls":[]}"#, "n100", 0.9, None).unwrap_err();
        assert_eq!(
            err,
            CompareError::Browser("no scripted CDP response for `Page.getLayoutMetrics`".into())
        );
        assert_eq!(
            err.to_string(),
            "no scripted CDP response for `Page.getLayoutMetrics`"
        );
    }

    #[ignore = "firewall pivot; re-home under guard"]
    #[test]
    fn non_finite_confidence_is_exact_and_bad_ids_do_not_parse_as_a_script_error() {
        let script = include_str!("../../../fixtures/sign-in-press.cdp.json");
        let err = scored_press(script, "n100", f64::NAN, None).unwrap_err();
        assert_eq!(err, CompareError::NonFiniteConfidence);
        assert_eq!(err.to_string(), "confidence must be finite");
        let err = scored_press("{}", "bad id", 0.9, None).unwrap_err();
        assert_eq!(err, CompareError::UnknownRegion("bad id".into()));
        assert_eq!(err.to_string(), "unknown region `bad id`");
    }

    #[test]
    fn a_manifold_file_cannot_act_and_an_empty_viewport_cannot_rank() {
        let sidebar = include_str!("../../../fixtures/sidebar.manifold");
        let spec = Spec {
            file: "sidebar.manifold",
            text: "Settings",
            role: Some(Role::Button),
            position: Some(Zone::Left),
            act: true,
        };
        let err = build_case(spec, sidebar).unwrap_err();
        assert_eq!(
            err,
            CompareError::ManifoldCannotAct {
                fixture: "sidebar.manifold".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "sidebar.manifold is a manifold fixture and cannot act"
        );
        let empty = parse_fixture("viewport w=10 h=10\n").unwrap();
        let err = top_match("empty.manifold", &LocateQuery::new(), &empty).unwrap_err();
        assert_eq!(
            err,
            CompareError::EmptyRank {
                fixture: "empty.manifold".into(),
            }
        );
        assert_eq!(err.to_string(), "empty.manifold has no regions to rank");
    }

    #[test]
    fn missing_fixture_is_an_io_error() {
        let dir =
            std::env::temp_dir().join(format!("ultra-instinct-compare-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let err = fixture_compare(&dir).unwrap_err();
        match err {
            CompareError::Io { path, message } => {
                assert!(path.ends_with("sidebar.manifold"), "{path}");
                assert!(message.contains("os error 2"), "{message}");
            }
            other => panic!("unexpected {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn agreement_is_id_or_press_equality() {
        assert!(agrees_press("press", true));
        assert!(!agrees_press("press", false));
        assert!(agrees_press("do-not-press", false));
        assert!(!agrees_press("do-not-press", true));
        assert!(!agrees_press("maybe", true));
        let located = FixtureCase {
            fixture: "sign-in.cdp.json".into(),
            region_id: "n100".into(),
            confidence: 0.9,
            executed: None,
            jev_choice: None,
            agree: None,
        };
        assert!(agrees(&located, "n100"));
        assert!(!agrees(&located, "n200"));
    }

    #[test]
    fn prompts_name_both_press_options_and_omit_the_gate() {
        let sidebar = parse_fixture(include_str!("../../../fixtures/sidebar.manifold")).unwrap();
        let region = sidebar.get_str("nav-settings").unwrap();
        let prompt = press_prompt(region, 0.9);
        assert!(prompt.contains("press"));
        assert!(prompt.contains("do-not-press"));
        assert!(!prompt.contains("0.55"));
        assert!(!prompt.contains("threshold"));
        assert!(!prompt.contains("correct"));
        let region_question = region_prompt("Settings");
        assert_eq!(
            region_question,
            "Which region id matches the Settings control?"
        );
        assert!(!region_question.contains("nav-settings"));
        let described = describe_region(region);
        assert!(described.contains("button"));
        assert!(described.contains("Settings"));
        assert!(described.contains("x="));
    }

    #[test]
    fn eval_corpus_reports_weighted_without_a_winner() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/locate");
        let report = eval_corpus(&dir).unwrap();
        let rendered = report.render();
        assert!(!rendered.contains("winner"), "{rendered}");
        assert_eq!(report.cases().len(), 5, "{rendered}");
        let mut seen = Vec::new();
        for case in report.cases() {
            assert_eq!(case.hits().len(), 1, "{rendered}");
            assert_eq!(case.hits()[0].matcher(), "weighted");
            assert_eq!(
                case.hits()[0].top_id(),
                case.expected_id(),
                "weighted top for {} / {}\n{rendered}",
                case.fixture(),
                case.text()
            );
            seen.push((
                case.text().to_owned(),
                case.hits()[0].top_id().to_owned(),
                case.hits()[0].margin_millis(),
                case.hits()[0].gate_would_refuse(),
            ));
        }
        assert_eq!(
            seen.iter().map(|row| row.0.as_str()).collect::<Vec<_>>(),
            ["Send", "Settings", "Export", "Admin", "Undo"]
        );
        assert_eq!(
            seen,
            vec![
                ("Send".into(), "z-send".into(), Some(125), false),
                ("Settings".into(), "nav-settings".into(), Some(300), false),
                ("Export".into(), "z-export".into(), Some(250), false),
                ("Admin".into(), "z-admin".into(), Some(449), false),
                ("Undo".into(), "z-undo".into(), Some(350), false),
            ]
        );
    }

    #[test]
    fn compare_errors_are_exact() {
        use ultra_instinct_core::Rect;

        let missing = std::env::temp_dir().join("ultra-instinct-missing-fixtures");
        let err = fixture_compare(&missing).unwrap_err();
        assert!(matches!(err, CompareError::Io { .. }), "{err:?}");

        let err = CompareError::Fixture("broken".into());
        assert_eq!(err.to_string(), "fixture: broken");
        let err = CompareError::Locate("empty text".into());
        assert_eq!(err.to_string(), "locate: empty text");
        let err = CompareError::Corpus {
            path: "cases.tsv".into(),
            message: "no cases".into(),
        };
        assert_eq!(err.to_string(), "eval corpus cases.tsv: no cases");
        assert_eq!(
            CompareError::TransportCalledBelowThreshold.to_string(),
            "scored confidence is below 0.55 but the transport was called"
        );

        // EmptyRank through a real empty manifold.
        let page = InteractionManifold::try_new(
            Rect::try_viewport(0.0, 0.0, 100.0, 100.0).unwrap(),
            Vec::new(),
            0,
        )
        .unwrap();
        let ranked = WeightedMatcher::default()
            .rank(&LocateQuery::new().text("x").unwrap(), &page)
            .unwrap();
        assert!(ranked.is_empty());
        assert_eq!(
            CompareError::EmptyRank {
                fixture: "empty".into()
            }
            .to_string(),
            "empty has no regions to rank"
        );

        // TransportCalledBelowThreshold is unreachable with BrowserExecutor:
        // gate_confidence runs before press, so a refusal never logs a CDP call.
        // The Display assert above is the owner for that variant.
    }
}
