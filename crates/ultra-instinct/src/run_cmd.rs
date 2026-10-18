//! `ultra-instinct run`: the owned agent loop from the command line.
//!
//! ```text
//! ultra-instinct run --goal <text> --cdp [url] [--url <page>] [--max-steps N] [--wait-secs N]
//! ultra-instinct run --goal <text> --fixture <replay.cdp.json>   # full loop over a CDP replay
//! ultra-instinct run --goal <text> --fixture <page.manifold>     # predict only (dry run)
//! ultra-instinct run ... --text-model-cmd <program>   # feature `model-text`: model TYPE/SELECT payloads
//! ultra-instinct run ... --policy jev                 # feature `jev`: JEV decides every step (default)
//! ultra-instinct run ... --policy jev --no-screenshot # JEV without the viewport screenshot
//! ultra-instinct run ... --policy jev --no-page-text  # JEV / Clef without the page's text
//! ultra-instinct run ... --no-derived-text            # feature `model-text`: never type values read off the page
//! ultra-instinct run ... --policy clef                # feature `clef`: Cloudflare Clef (27B) decides
//! ultra-instinct run ... --policy clef-flash          # feature `clef`: Cloudflare Clef-Flash (9B) decides
//! ultra-instinct run ... --policy instinct            # offline; `pua` is a deprecated alias
//! ```
//!
//! Live mode with `--url` opens an owned background tab; it closes when the run
//! ends, after the output records the page's `final_url`. Without `--url`, it drives the first existing Chrome page. The loop is
//! observe → policy → gate → ticket → executor (revalidate + consume) → input →
//! observe → verify, until DONE, BLOCKED, abstain, or a bound. With the `jev`
//! feature the default policy is JEV (System One). `--policy instinct` is the
//! offline policy: no LLM and no MCP are involved; Instinct abstains rather than
//! guessing. `--policy pua` remains a deprecated alias for `--policy instinct`.
//!
//! `--policy jev` (built with `--features jev`) decides through JEV (System
//! One) instead: the offered ActionSpace becomes one `operation` + one
//! `<kind>_target` question per element kind in a single call — the
//! speculative fan-out jev-ultrafast uses. JEV can only pick an offered id,
//! never a selector or script, and the ticket / gate / revalidate chain is
//! unchanged. Live runs send each decision's viewport screenshot (JPEG,
//! `Page.captureScreenshot` on the agent's own socket) as a System One image,
//! so JEV can read values that exist only on the page; `--no-screenshot`
//! turns that off. Reads `TYPESAFE_API_KEY` (required), `TYPESAFE_MODEL`,
//! `TYPESAFE_BASE_URL`. It does not read `TEXT_MODEL_*` yet; combine with
//! `--text-model-cmd` for model-written payloads.
//!
//! `--policy clef` / `--policy clef-flash` (built with `--features clef`) decide
//! through Cloudflare Workers AI Clef models — same closed wire and the same
//! System One body as `jev`, sent by `typesafe-sdk`'s Clef provider to
//! `…/accounts/{id}/ai/run/@cf/cloudflare/{model}`. Reads `CLOUDFLARE_ACCOUNT_ID`,
//! `CLOUDFLARE_API_TOKEN` (required), `CLEF_MODEL`, `CLEF_BASE_URL` (the prefix
//! before `/accounts`). A target head with one offered candidate never reaches the
//! wire — it is answered locally.
//!
//! `--text-model-cmd` (built with `--features model-text`) only changes where
//! TYPE_TEXT / SELECT *payloads* come from: a user program speaking the
//! `CommandTextModel` JSON line protocol. Replies are context-bound and
//! grounded in the goal; refused replies fall back to the deterministic
//! resolver, then abstain. The program owns any API keys. Without the flag,
//! jev-ultrafast's `TEXT_MODEL_API_KEY`/`TEXT_MODEL_BASE_URL`/`TEXT_MODEL`
//! env drives an HTTP model — OpenAI-compatible, Anthropic, or Gemini wire,
//! picked by `TEXT_MODEL_API` or the base URL host ([`crate::http_text`]).

use ultra_instinct_agent::{Agent, AgentBuilder, AgentOutcome, BrowserRuntime, MockBrowser};
use ultra_instinct_browser::{
    open_tab, BrowserSession, CdpTransport, ReplayTransport, WebSocketTransport,
};
use ultra_instinct_core::parse_fixture;
use ultra_instinct_policy::{BrowserPolicy, InstinctPolicy, TextResolver};

use crate::CliError;

struct RunArgs {
    goal: String,
    cdp: Option<String>,
    url: Option<String>,
    fixture: Option<String>,
    max_steps: u32,
    wait_secs: Option<u32>,
    text_model_cmd: Option<String>,
    policy: PolicyKind,
    /// `--diary <dir>`: write the run's battle diary (schema v1 JSONL).
    diary: Option<String>,
    /// `--lessons <path>`: the dojo's lesson store (loaded at start,
    /// grown during the run, saved back at exit).
    lessons: Option<String>,
    /// `--no-screenshot`: a pixel-reading policy decides without the viewport.
    /// Only `jev` reads pixels today.
    #[cfg_attr(not(feature = "jev"), allow(dead_code))]
    no_screenshot: bool,
    /// `--no-page-text`: remote policies decide without the page's text.
    #[cfg_attr(not(feature = "jev"), allow(dead_code))]
    no_page_text: bool,
    /// `--allow-derived-text`: require derived text (an error without a
    /// text model). Derived text is on by default whenever one is configured.
    #[cfg_attr(not(feature = "model-text"), allow(dead_code))]
    allow_derived_text: bool,
    /// `--no-derived-text`: the text model types only values the goal names.
    #[cfg_attr(not(feature = "model-text"), allow(dead_code))]
    no_derived_text: bool,
    /// `--second-opinion`: ask the text model (`TEXT_MODEL_*`) to choose
    /// when JEV is unsure (features `jev` + `model-text`).
    #[cfg_attr(not(feature = "jev"), allow(dead_code))]
    second_opinion: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PolicyKind {
    Instinct,
    Jev,
    Clef,
    ClefFlash,
    /// Instinct first; learned-move replay next; remote last. Writes
    /// verified remote wins back into the --lessons store.
    Dojo,
}

fn parse(args: &[String]) -> Result<RunArgs, CliError> {
    let mut goal = None;
    let mut cdp = None;
    let mut url = None;
    let mut fixture = None;
    let mut max_steps = None;
    let mut wait_secs = None;
    let mut text_model_cmd = None;
    let mut policy = None;
    let mut diary = None;
    let mut lessons = None;
    let mut no_screenshot = false;
    let mut no_page_text = false;
    let mut allow_derived_text = false;
    let mut no_derived_text = false;
    let mut second_opinion = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
            _ => (arg, None),
        };
        let mut value = |flag: &'static str| -> Result<String, CliError> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            i += 1;
            match args.get(i) {
                Some(v) if !v.starts_with("--") => Ok(v.clone()),
                _ => Err(CliError::MissingValue(flag)),
            }
        };
        match name {
            "--goal" => set(&mut goal, "--goal", value("--goal")?)?,
            "--url" => set(&mut url, "--url", value("--url")?)?,
            "--fixture" => set(&mut fixture, "--fixture", value("--fixture")?)?,
            "--text-model-cmd" => set(
                &mut text_model_cmd,
                "--text-model-cmd",
                value("--text-model-cmd")?,
            )?,
            "--policy" => {
                let raw = value("--policy")?;
                let kind = match raw.as_str() {
                    "instinct" | "pua" => PolicyKind::Instinct,
                    "jev" => PolicyKind::Jev,
                    "clef" => PolicyKind::Clef,
                    "clef-flash" => PolicyKind::ClefFlash,
                    "dojo" => PolicyKind::Dojo,
                    _ => {
                        return Err(CliError::UnknownFlag(format!(
                            "unknown policy `{raw}` (instinct|jev|clef|clef-flash|dojo)"
                        )))
                    }
                };
                set(&mut policy, "--policy", kind)?;
            }
            "--lessons" => set(&mut lessons, "--lessons", value("--lessons")?)?,
            "--no-screenshot" => switch(&mut no_screenshot, "--no-screenshot", arg, &inline)?,
            "--no-page-text" => switch(&mut no_page_text, "--no-page-text", arg, &inline)?,
            "--allow-derived-text" => switch(
                &mut allow_derived_text,
                "--allow-derived-text",
                arg,
                &inline,
            )?,
            "--no-derived-text" => switch(&mut no_derived_text, "--no-derived-text", arg, &inline)?,
            "--second-opinion" => switch(&mut second_opinion, "--second-opinion", arg, &inline)?,
            "--max-steps" => {
                let raw = value("--max-steps")?;
                let n: u32 = raw.parse().map_err(|_| CliError::BadNumber {
                    flag: "--max-steps",
                    value: raw.clone(),
                })?;
                set(&mut max_steps, "--max-steps", n)?;
            }
            "--wait-secs" => {
                let raw = value("--wait-secs")?;
                let n: u32 = raw.parse().map_err(|_| CliError::BadNumber {
                    flag: "--wait-secs",
                    value: raw.clone(),
                })?;
                set(&mut wait_secs, "--wait-secs", n)?;
            }
            "--diary" => set(&mut diary, "--diary", value("--diary")?)?,
            "--cdp" => {
                if cdp.is_some() {
                    return Err(CliError::DuplicateFlag("--cdp"));
                }
                if let Some(v) = inline.clone() {
                    cdp = Some(v);
                } else if args.get(i + 1).is_some_and(|v| !v.starts_with("--")) {
                    i += 1;
                    cdp = Some(args[i].clone());
                } else {
                    cdp = Some(ultra_instinct_browser::DEFAULT_CDP_HTTP.to_owned());
                }
            }
            _ => return Err(CliError::UnknownFlag(arg.to_owned())),
        }
        i += 1;
    }
    let goal = goal.ok_or(CliError::MissingValue("--goal"))?;
    if goal.trim().is_empty() {
        return Err(CliError::EmptyText);
    }
    if cdp.is_some() && fixture.is_some() {
        return Err(CliError::DuplicateFlag("--cdp"));
    }
    if cdp.is_none() && fixture.is_none() {
        return Err(CliError::MissingSource);
    }
    if url.is_some() && cdp.is_none() {
        return Err(CliError::UnknownFlag("--url requires --cdp".into()));
    }
    if allow_derived_text && no_derived_text {
        return Err(CliError::UnknownFlag(
            "--allow-derived-text and --no-derived-text contradict each other".into(),
        ));
    }
    Ok(RunArgs {
        goal,
        cdp,
        url,
        fixture,
        max_steps: max_steps.unwrap_or(20),
        wait_secs,
        text_model_cmd,
        #[cfg(feature = "jev")]
        policy: policy.unwrap_or(PolicyKind::Jev),
        #[cfg(not(feature = "jev"))]
        policy: policy.unwrap_or(PolicyKind::Instinct),
        diary,
        lessons,
        no_screenshot,
        no_page_text,
        allow_derived_text,
        no_derived_text,
        second_opinion,
    })
}

/// A bare on switch: no value, at most once.
fn switch(
    slot: &mut bool,
    flag: &'static str,
    arg: &str,
    inline: &Option<String>,
) -> Result<(), CliError> {
    if inline.is_some() {
        return Err(CliError::UnknownFlag(arg.to_owned()));
    }
    if *slot {
        return Err(CliError::DuplicateFlag(flag));
    }
    *slot = true;
    Ok(())
}

fn set<T>(slot: &mut Option<T>, flag: &'static str, value: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::DuplicateFlag(flag));
    }
    *slot = Some(value);
    Ok(())
}

pub(crate) fn run_command(args: &[String]) -> Result<String, CliError> {
    let args = parse(args)?;
    if let Some(endpoint) = &args.cdp {
        let transport = if args.url.is_some() {
            open_tab(endpoint)
        } else {
            WebSocketTransport::connect(endpoint)
        }
        .map_err(|err| CliError::Browser(err.to_string()))?;
        let mut session = BrowserSession::new(transport);
        if let Some(url) = &args.url {
            session
                .navigate(url)
                .map_err(|err| CliError::Browser(err.to_string()))?;
            BrowserRuntime::settle(&mut session);
        }
        return drive(session, &args);
    }
    let path = args.fixture.as_deref().expect("checked in parse");
    let body = std::fs::read_to_string(path).map_err(|err| CliError::Io {
        path: path.to_owned(),
        message: err.to_string(),
    })?;
    if body.trim_start().starts_with('{') {
        let transport =
            ReplayTransport::parse(&body).map_err(|err| CliError::Browser(err.to_string()))?;
        return drive(BrowserSession::new(transport), &args);
    }
    // Static manifold: predict once. Executing against a page that cannot
    // change would only measure the mock.
    let manifold = parse_fixture(&body).map_err(|err| CliError::Fixture(err.to_string()))?;
    match args.policy {
        PolicyKind::Instinct => {
            predict_once(MockBrowser::new(manifold), InstinctPolicy::default(), &args)
        }
        PolicyKind::Jev => predict_jev(manifold, &args),
        PolicyKind::Clef => predict_clef(manifold, &args, "clef"),
        PolicyKind::ClefFlash => predict_clef(manifold, &args, "clef-flash"),
        PolicyKind::Dojo => predict_dojo(manifold, &args),
    }
}

fn predict_dojo(
    manifold: ultra_instinct_core::InteractionManifold,
    args: &RunArgs,
) -> Result<String, CliError> {
    predict_once(MockBrowser::new(manifold), dojo_policy(args)?, args)
}

fn predict_once<P: BrowserPolicy>(
    browser: MockBrowser,
    policy: P,
    args: &RunArgs,
) -> Result<String, CliError> {
    let mut agent = AgentBuilder::new(browser, policy)
        .max_steps(1)
        .build(args.goal.clone());
    let mut out = String::from("mode dry-run (static manifold; nothing executed)\n");
    match agent.predict() {
        Ok(Some(p)) => {
            out.push_str(&format!(
                "predict {} {:?} confidence={} payload={}\n",
                p.decision.action_id,
                p.decision.target_label,
                p.decision.confidence_millis,
                p.payload
                    .as_deref()
                    .map_or("-".to_owned(), |t| format!("{t:?}"))
            ));
        }
        Ok(None) => out.push_str(&format!("predict terminal {:?}\n", agent.state())),
        Err(err) => out.push_str(&format!("predict refused: {err}\n")),
    }
    if let Some(line) = write_diary_line(&mut agent, args)? {
        out.push_str(&line);
    }
    Ok(out)
}

#[cfg(feature = "jev")]
fn predict_jev(
    manifold: ultra_instinct_core::InteractionManifold,
    args: &RunArgs,
) -> Result<String, CliError> {
    // A manifold fixture has no pixels or page text to send.
    let transport = crate::typesafe::TypesafeTransport::from_env(false, false)
        .map_err(|err| CliError::Config(format!("typesafe: {err}")))?;
    predict_once(
        MockBrowser::new(manifold),
        ultra_instinct_policy::RemotePolicy::named("jev", transport),
        args,
    )
}

#[cfg(not(feature = "jev"))]
fn predict_jev(
    _manifold: ultra_instinct_core::InteractionManifold,
    _args: &RunArgs,
) -> Result<String, CliError> {
    Err(CliError::UnknownFlag(
        "--policy jev requires building with --features jev".into(),
    ))
}

#[cfg(feature = "clef")]
fn predict_clef(
    manifold: ultra_instinct_core::InteractionManifold,
    args: &RunArgs,
    model: &str,
) -> Result<String, CliError> {
    let transport = crate::clef::ClefTransport::from_env(model)
        .map_err(|err| CliError::Config(format!("clef: {err}")))?;
    let model_name: &'static str = if model == "clef-flash" {
        "clef-flash"
    } else {
        "clef"
    };
    predict_once(
        MockBrowser::new(manifold),
        ultra_instinct_policy::RemotePolicy::named(model_name, transport),
        args,
    )
}

#[cfg(not(feature = "clef"))]
fn predict_clef(
    _manifold: ultra_instinct_core::InteractionManifold,
    _args: &RunArgs,
    _model: &str,
) -> Result<String, CliError> {
    Err(CliError::UnknownFlag(
        "--policy clef requires building with --features clef".into(),
    ))
}

fn drive<T: CdpTransport>(session: BrowserSession<T>, args: &RunArgs) -> Result<String, CliError> {
    match args.policy {
        PolicyKind::Instinct => drive_with(session, InstinctPolicy::default(), args),
        PolicyKind::Jev => drive_jev(session, args),
        PolicyKind::Clef => drive_clef(session, args, "clef"),
        PolicyKind::ClefFlash => drive_clef(session, args, "clef-flash"),
        PolicyKind::Dojo => drive_dojo(session, args),
    }
}

/// The dojo's remote arm: JEV when its transport is configured, else the
/// run decides locally only (no remote calls, nothing new is learned
/// remotely — but distilled lessons still replay).
fn dojo_remote() -> Option<Box<dyn BrowserPolicy>> {
    #[cfg(feature = "jev")]
    {
        // The dojo does not forward screenshots to its remote arm yet.
        if let Ok(transport) = crate::typesafe::TypesafeTransport::from_env(false, false) {
            return Some(Box::new(ultra_instinct_policy::RemotePolicy::named(
                "jev", transport,
            )));
        }
    }
    None
}

fn dojo_policy(args: &RunArgs) -> Result<crate::dojo_policy::DojoPolicy, CliError> {
    let store = match &args.lessons {
        Some(path) => {
            ultra_instinct_dojo::load_lessons(std::path::Path::new(path)).map_err(|e| {
                CliError::Io {
                    path: path.clone(),
                    message: e.to_string(),
                }
            })?
        }
        None => ultra_instinct_dojo::LessonStore::default(),
    };
    Ok(crate::dojo_policy::DojoPolicy::new(dojo_remote(), store))
}

fn drive_dojo<T: CdpTransport>(
    session: BrowserSession<T>,
    args: &RunArgs,
) -> Result<String, CliError> {
    let mut builder = AgentBuilder::new(session, dojo_policy(args)?).max_steps(args.max_steps);
    if let Some(secs) = args.wait_secs {
        let polls = u64::from(secs) * 1_000 / ultra_instinct_agent::WAIT_POLL_MS;
        builder = builder
            .max_wait_polls(u32::try_from(polls).unwrap_or(u32::MAX))
            .max_wait(std::time::Duration::from_secs(u64::from(secs)));
    }
    let mut agent = builder.build(args.goal.clone());
    let outcome = agent.run();
    let diary_path = write_run_diary(&mut agent, args)?;
    let diary = diary_path
        .as_ref()
        .map(|p| format!("diary: {}\n", p.display()));
    let mut out = with_diary(render(&agent, &outcome), diary.as_deref())?;
    let policy = agent.into_policy();
    let credited = policy.credited_steps().to_vec();
    let learned = policy.learned_count();
    let mut store = policy.into_store();
    // Steps credited live already hold their trust — mark them so an
    // offline `lessons` distill of this diary never counts them twice.
    if let Some(path) = diary_path.as_ref() {
        let diary = ultra_instinct_dojo::diary_id(path);
        for seq in &credited {
            store.seen_steps.insert(format!("{diary}:{seq}"));
        }
    }
    if let Some(path) = &args.lessons {
        ultra_instinct_dojo::save_lessons(&store, std::path::Path::new(path)).map_err(|e| {
            CliError::Io {
                path: path.clone(),
                message: e.to_string(),
            }
        })?;
        out.push_str(&format!("lessons {path}: store saved\n"));
    } else if learned > 0 {
        out.push_str("dojo: lessons grown in memory only (no --lessons store to save)\n");
    }
    Ok(out)
}

fn drive_with<T: CdpTransport, P: BrowserPolicy>(
    session: BrowserSession<T>,
    policy: P,
    args: &RunArgs,
) -> Result<String, CliError> {
    let mut builder = AgentBuilder::new(session, policy).max_steps(args.max_steps);
    if let Some(secs) = args.wait_secs {
        // One re-check per WAIT_POLL_MS.
        let polls = u64::from(secs) * 1_000 / ultra_instinct_agent::WAIT_POLL_MS;
        builder = builder
            .max_wait_polls(u32::try_from(polls).unwrap_or(u32::MAX))
            .max_wait(std::time::Duration::from_secs(u64::from(secs)));
    }
    if let Some(program) = args.text_model_cmd.as_deref() {
        return drive_model_text_cmd(builder, program, args);
    }
    // jev-ultrafast parity: no flag but TEXT_MODEL_API_KEY set means the
    // HTTP model (OpenAI-compatible, Anthropic, or Gemini wire) fills
    // payloads. The flag always wins.
    #[cfg(feature = "model-text")]
    if let Some(model) = crate::http_text::HttpTextModel::from_env().map_err(CliError::Config)? {
        let source = model.api().label();
        return drive_model_text(builder, source, Box::new(model), args);
    }
    if args.allow_derived_text {
        return Err(CliError::Config(
            "--allow-derived-text needs a text model (TEXT_MODEL_API_KEY or --text-model-cmd)"
                .into(),
        ));
    }
    let mut agent = builder.build(args.goal.clone());
    let outcome = agent.run();
    let diary = write_diary_line(&mut agent, args)?;
    with_diary(render(&agent, &outcome), diary.as_deref())
}

#[cfg(feature = "jev")]
fn drive_jev<T: CdpTransport>(
    session: BrowserSession<T>,
    args: &RunArgs,
) -> Result<String, CliError> {
    let transport =
        crate::typesafe::TypesafeTransport::from_env(!args.no_screenshot, !args.no_page_text)
            .map_err(|err| CliError::Config(format!("typesafe: {err}")))?;
    let transport = if args.second_opinion {
        with_second_opinion(transport)?
    } else {
        transport
    };
    drive_with(
        session,
        ultra_instinct_policy::RemotePolicy::named("jev", transport),
        args,
    )
}

/// The JEV transport with the text model as its second opinion.
#[cfg(all(feature = "jev", feature = "model-text"))]
fn with_second_opinion(
    transport: crate::typesafe::TypesafeTransport,
) -> Result<crate::typesafe::TypesafeTransport, CliError> {
    let advisor = crate::second::SecondOpinion::from_env()
        .map_err(CliError::Config)?
        .ok_or_else(|| {
            CliError::Config("--second-opinion needs a text model (TEXT_MODEL_API_KEY)".into())
        })?;
    Ok(transport.with_advisor(Box::new(advisor)))
}

#[cfg(all(feature = "jev", not(feature = "model-text")))]
fn with_second_opinion(
    _transport: crate::typesafe::TypesafeTransport,
) -> Result<crate::typesafe::TypesafeTransport, CliError> {
    Err(CliError::UnknownFlag(
        "--second-opinion requires building with --features jev,model-text".into(),
    ))
}

#[cfg(not(feature = "jev"))]
fn drive_jev<T: CdpTransport>(
    _session: BrowserSession<T>,
    _args: &RunArgs,
) -> Result<String, CliError> {
    Err(CliError::UnknownFlag(
        "--policy jev requires building with --features jev".into(),
    ))
}

#[cfg(feature = "clef")]
fn drive_clef<T: CdpTransport>(
    session: BrowserSession<T>,
    args: &RunArgs,
    model: &str,
) -> Result<String, CliError> {
    let transport = crate::clef::ClefTransport::from_env(model)
        .map_err(|err| CliError::Config(format!("clef: {err}")))?
        .with_page_text(!args.no_page_text);
    let model_name: &'static str = if model == "clef-flash" {
        "clef-flash"
    } else {
        "clef"
    };
    drive_with(
        session,
        ultra_instinct_policy::RemotePolicy::named(model_name, transport),
        args,
    )
}

#[cfg(not(feature = "clef"))]
fn drive_clef<T: CdpTransport>(
    _session: BrowserSession<T>,
    _args: &RunArgs,
    _model: &str,
) -> Result<String, CliError> {
    Err(CliError::UnknownFlag(
        "--policy clef requires building with --features clef".into(),
    ))
}

#[cfg(feature = "model-text")]
fn drive_model_text_cmd<B: BrowserRuntime, P: BrowserPolicy>(
    builder: AgentBuilder<B, P>,
    program: &str,
    args: &RunArgs,
) -> Result<String, CliError> {
    drive_model_text(
        builder,
        "command",
        Box::new(ultra_instinct_policy::CommandTextModel::new(program)),
        args,
    )
}

#[cfg(not(feature = "model-text"))]
fn drive_model_text_cmd<B: BrowserRuntime, P: BrowserPolicy>(
    _builder: AgentBuilder<B, P>,
    _program: &str,
    _args: &RunArgs,
) -> Result<String, CliError> {
    Err(CliError::UnknownFlag(
        "--text-model-cmd requires building with --features model-text".into(),
    ))
}

#[cfg(feature = "model-text")]
fn drive_model_text<B: BrowserRuntime, P: BrowserPolicy>(
    builder: AgentBuilder<B, P>,
    source: &str,
    model: Box<dyn ultra_instinct_policy::TextModel>,
    args: &RunArgs,
) -> Result<String, CliError> {
    // Derived text (a value read or worked out from the page, guarded) is on
    // whenever a text model is configured, unless `--no-derived-text`.
    let resolver =
        ultra_instinct_policy::ModelTextResolver::new(model).allow_derived(!args.no_derived_text);
    let mut agent = builder.text_resolver(resolver).build(args.goal.clone());
    let outcome = agent.run();
    let mut out = format!(
        "text resolver model ({source}) calls={}\n",
        agent.text_resolver().model_calls()
    );
    // Every value typed from the page instead of the goal, for review.
    for value in agent.text_resolver().derived() {
        out.push_str(&format!("text derived from page: {value:?}\n"));
    }
    let diary = write_diary_line(&mut agent, args)?;
    match with_diary(render(&agent, &outcome), diary.as_deref()) {
        Ok(rendered) => {
            out.push_str(&rendered);
            Ok(out)
        }
        Err(CliError::Agent(rendered)) => {
            out.push_str(&rendered);
            Err(CliError::Agent(out))
        }
        Err(other) => Err(other),
    }
}

/// Write the run's diary (when `--diary <dir>`) and return its path;
/// `None` without the flag. Written even when the run's outcome is
/// Failed — the outcome line belongs in the diary too.
fn write_run_diary<B, P, T>(
    agent: &mut Agent<B, P, T>,
    args: &RunArgs,
) -> Result<Option<std::path::PathBuf>, CliError>
where
    B: BrowserRuntime,
    P: BrowserPolicy,
    T: TextResolver,
{
    let Some(dir) = args.diary.as_deref() else {
        return Ok(None);
    };
    let events = agent.take_journal();
    let path = crate::diary::write_diary(std::path::Path::new(dir), &events)?;
    Ok(Some(path))
}

/// [`write_run_diary`] rendered as the `diary: <path>` output line.
fn write_diary_line<B, P, T>(
    agent: &mut Agent<B, P, T>,
    args: &RunArgs,
) -> Result<Option<String>, CliError>
where
    B: BrowserRuntime,
    P: BrowserPolicy,
    T: TextResolver,
{
    Ok(write_run_diary(agent, args)?.map(|p| format!("diary: {}\n", p.display())))
}

/// Append the `diary: <path>` line to rendered output — including inside a
/// `CliError::Agent` transcript, so a failed run still records its path.
fn with_diary(rendered: Result<String, CliError>, diary: Option<&str>) -> Result<String, CliError> {
    let Some(diary) = diary else {
        return rendered;
    };
    match rendered {
        Ok(text) => Ok(format!("{text}{diary}")),
        Err(CliError::Agent(text)) => Err(CliError::Agent(format!("{text}{diary}"))),
        Err(other) => Err(other),
    }
}

fn render<B, P, T>(agent: &Agent<B, P, T>, outcome: &AgentOutcome) -> Result<String, CliError>
where
    B: BrowserRuntime,
    P: BrowserPolicy,
    T: TextResolver,
{
    let mut out = String::new();
    for step in outcome.steps() {
        out.push_str(&format!(
            "step {} {} {:?} -> {}\n",
            step.step,
            step.action_id,
            step.label,
            step.verification.as_str()
        ));
    }
    let (kind, why) = match outcome {
        AgentOutcome::Done { reason, .. } => ("done", reason.as_str()),
        AgentOutcome::Blocked { reason, .. } => ("blocked", reason.as_str()),
        AgentOutcome::Abstained { reason, .. } => ("abstained", reason.as_str()),
        AgentOutcome::Failed { error, .. } => ("failed", error.as_str()),
    };
    out.push_str(&format!(
        "outcome {kind}: {why}\npolicy_calls {} stale_discards {}\n",
        agent.policy_calls(),
        agent.stale_discards()
    ));
    if let Some(url) = agent.browser().page().and_then(|page| page.url()) {
        out.push_str(&format!("final_url {url}\n"));
    }
    if matches!(outcome, AgentOutcome::Failed { .. }) {
        return Err(CliError::Agent(out));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn requires_goal_and_source() {
        assert_eq!(
            run_command(&a(&["--cdp"])).unwrap_err(),
            CliError::MissingValue("--goal")
        );
        assert_eq!(
            run_command(&a(&["--goal", "Go"])).unwrap_err(),
            CliError::MissingSource
        );
        assert!(matches!(
            run_command(&a(&["--goal", "Go", "--fixture", "x", "--url", "http://a"])).unwrap_err(),
            CliError::UnknownFlag(_)
        ));
    }

    #[test]
    #[cfg(feature = "jev")]
    fn jev_policy_is_default_and_pua_is_an_alias() {
        let default = parse(&a(&["--goal", "Go", "--fixture", "x"])).unwrap();
        assert_eq!(default.policy, PolicyKind::Jev);
        for alias in ["instinct", "pua"] {
            let parsed = parse(&a(&["--goal", "Go", "--fixture", "x", "--policy", alias])).unwrap();
            assert_eq!(parsed.policy, PolicyKind::Instinct);
        }
    }

    #[cfg(not(feature = "jev"))]
    #[test]
    fn instinct_policy_is_default_and_pua_is_an_alias() {
        let default = parse(&a(&["--goal", "Go", "--fixture", "x"])).unwrap();
        assert_eq!(default.policy, PolicyKind::Instinct);
        for alias in ["instinct", "pua"] {
            let parsed = parse(&a(&["--goal", "Go", "--fixture", "x", "--policy", alias])).unwrap();
            assert_eq!(parsed.policy, PolicyKind::Instinct);
        }
    }

    #[test]
    fn replay_script_runs_full_loop() {
        use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
        let page = PageSpec::of(
            &[Control::text_field(
                10,
                100,
                "Search",
                (20.0, 20.0, 300.0, 28.0),
            )],
            "http://127.0.0.1/",
            "Search",
        );
        let script = ScriptBuilder::new()
            .observe(&page)
            .observe(&page)
            .dom_input(10)
            .observe(&page)
            .dom_read_value(10, "rust", "")
            .observe(&page);
        let dir = std::env::temp_dir().join(format!("hu-run-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("search.cdp.json");
        std::fs::write(&path, script.to_json()).unwrap();
        let out = run_command(&a(&[
            "--goal",
            r#"Type "rust" into Search"#,
            "--policy",
            "instinct",
            "--fixture",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(out.contains("TYPE_TEXT:"), "{out}");
        assert!(out.contains("-> success"), "{out}");
        assert!(out.contains("outcome done"), "{out}");
    }

    #[cfg(not(feature = "model-text"))]
    #[test]
    fn text_model_cmd_requires_feature() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/agent-type-search.cdp.json");
        let err = run_command(&a(&[
            "--goal",
            "type rust in the Search box",
            "--policy",
            "instinct",
            "--fixture",
            path.to_str().unwrap(),
            "--text-model-cmd",
            "/nonexistent",
        ]))
        .unwrap_err();
        assert!(
            matches!(err, CliError::UnknownFlag(ref m) if m.contains("model-text")),
            "{err:?}"
        );
    }

    #[cfg(not(feature = "jev"))]
    #[test]
    fn policy_jev_requires_feature() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/agent-type-search.cdp.json");
        let err = run_command(&a(&[
            "--goal",
            "type rust in the Search box",
            "--fixture",
            path.to_str().unwrap(),
            "--policy",
            "jev",
        ]))
        .unwrap_err();
        assert!(
            matches!(err, CliError::UnknownFlag(ref m) if m.contains("--features jev")),
            "{err:?}"
        );
    }

    #[test]
    fn evidence_switches_are_bare_and_single() {
        let parsed = parse(&a(&["--goal", "Go", "--fixture", "x", "--no-page-text"])).unwrap();
        assert!(parsed.no_page_text && !parsed.no_screenshot && !parsed.allow_derived_text);
        let parsed = parse(&a(&[
            "--goal",
            "Go",
            "--fixture",
            "x",
            "--allow-derived-text",
        ]))
        .unwrap();
        assert!(parsed.allow_derived_text);
        assert!(parse(&a(&["--goal", "Go", "--fixture", "x", "--no-page-text=1"])).is_err());
        let parsed = parse(&a(&["--goal", "Go", "--fixture", "x", "--no-derived-text"])).unwrap();
        assert!(parsed.no_derived_text && !parsed.allow_derived_text);
        assert!(parse(&a(&[
            "--goal",
            "Go",
            "--fixture",
            "x",
            "--allow-derived-text",
            "--no-derived-text"
        ]))
        .is_err());
    }

    #[test]
    fn no_screenshot_is_a_bare_switch() {
        assert!(
            !parse(&a(&["--goal", "Go", "--fixture", "x"]))
                .unwrap()
                .no_screenshot
        );
        assert!(
            parse(&a(&["--goal", "Go", "--fixture", "x", "--no-screenshot"]))
                .unwrap()
                .no_screenshot
        );
        assert!(parse(&a(&["--goal", "Go", "--fixture", "x", "--no-screenshot=1"])).is_err());
        assert!(parse(&a(&[
            "--goal",
            "Go",
            "--fixture",
            "x",
            "--no-screenshot",
            "--no-screenshot"
        ]))
        .is_err());
    }

    #[test]
    fn unknown_policy_is_rejected() {
        let err =
            run_command(&a(&["--goal", "x", "--fixture", "y", "--policy", "grok"])).unwrap_err();
        assert!(
            matches!(err, CliError::UnknownFlag(ref m) if m.contains("instinct|jev")),
            "{err:?}"
        );
    }

    /// Full replay loop with a local scripted model command (no network).
    #[cfg(all(feature = "model-text", unix))]
    #[test]
    fn text_model_cmd_drives_replay_loop() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let fixture = root.join("agent-type-search.cdp.json");
        let dir = std::env::temp_dir().join(format!("hu-run-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("model.sh");
        std::fs::write(
            &model,
            "#!/bin/sh\nread -r line\nfp=$(printf '%s' \"$line\" | sed 's/.*\"context_fingerprint\":\\([0-9]*\\).*/\\1/')\nprintf '{\"text\":\"rust\",\"context_fingerprint\":%s}\\n' \"$fp\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&model, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = run_command(&a(&[
            "--goal",
            "type rust in the Search box",
            "--policy",
            "instinct",
            "--fixture",
            fixture.to_str().unwrap(),
            "--text-model-cmd",
            model.to_str().unwrap(),
        ]))
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            out.contains("text resolver model (command) calls=1"),
            "{out}"
        );
        assert!(out.contains("TYPE_TEXT:"), "{out}");
        assert!(out.contains("-> success"), "{out}");
        assert!(out.contains("outcome done"), "{out}");
    }

    #[test]
    fn static_manifold_is_dry_run_predict() {
        let dir = std::env::temp_dir().join(format!("hu-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("page.manifold");
        std::fs::write(
            &path,
            "viewport w=800 h=600\nregion id=go role=button label=\"Continue\" x=10 y=10 w=80 h=24 actions=click sources=dom,accessibility\n",
        )
        .unwrap();
        let out = run_command(&a(&[
            "--goal",
            "Continue",
            "--policy",
            "instinct",
            "--fixture",
            path.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(out.contains("dry-run"), "{out}");
        assert!(out.contains("CLICK:go"), "{out}");
    }

    #[test]
    fn checked_in_agent_fixtures_run_full_loop() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        let cases = [
            (
                r#"Type "rust" into Search"#,
                "agent-type-search.cdp.json",
                "TYPE_TEXT:",
            ),
            ("Click Go", "agent-click-go.cdp.json", "CLICK:"),
            (
                r#"Select "Business" in Cabin class"#,
                "agent-select-cabin.cdp.json",
                "SELECT:",
            ),
        ];
        for (goal, file, needle) in cases {
            let path = root.join(file);
            if !path.exists() {
                // Fixtures are generated once via WRITE_FIXTURES=1; skip until present
                // so a fresh checkout mid-PR still compiles.
                eprintln!("skip missing fixture {}", path.display());
                continue;
            }
            let out = run_command(&a(&[
                "--goal",
                goal,
                "--policy",
                "instinct",
                "--fixture",
                path.to_str().unwrap(),
            ]))
            .unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(out.contains(needle), "{file}: {out}");
            assert!(
                out.contains("outcome done")
                    || out.contains("-> success")
                    || out.contains("-> state-changed"),
                "{file}: {out}"
            );
        }
    }

    #[test]
    fn write_agent_loop_fixtures() {
        // Opt-in: WRITE_FIXTURES=1 cargo test -p ultra-instinct write_agent_loop_fixtures -- --ignored
        if std::env::var_os("WRITE_FIXTURES").is_none() {
            return;
        }
        use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
        // TYPE_TEXT full loop
        let search = PageSpec::of(
            &[Control::text_field(
                10,
                100,
                "Search",
                (20.0, 20.0, 300.0, 28.0),
            )],
            "http://127.0.0.1/search",
            "Search",
        );
        let type_script = ScriptBuilder::new()
            .observe(&search)
            .observe(&search)
            .dom_input(10)
            .observe(&search)
            .dom_read_value(10, "rust", "")
            .observe(&search);
        std::fs::write(
            root.join("agent-type-search.cdp.json"),
            type_script.to_json(),
        )
        .unwrap();
        // CLICK full loop
        let click_page = PageSpec::of(
            &[Control::button(20, 200, "Go", (20.0, 60.0, 80.0, 28.0))],
            "http://127.0.0.1/go",
            "Go",
        );
        let after = PageSpec::of(
            &[Control::button(21, 210, "Done", (20.0, 60.0, 80.0, 28.0))],
            "http://127.0.0.1/go",
            "Done",
        );
        let click_script = ScriptBuilder::new()
            .observe(&click_page)
            .observe(&click_page)
            .pointer_click()
            .observe(&after)
            .observe(&after);
        std::fs::write(root.join("agent-click-go.cdp.json"), click_script.to_json()).unwrap();
        // SELECT full loop
        let select_ctrl = Control {
            node_id: 30,
            backend: 300,
            tag: "SELECT",
            role: "combobox",
            label: "Cabin class".into(),
            rect: (20.0, 20.0, 200.0, 28.0),
            focused: false,
        };
        let select_page = PageSpec::of(&[select_ctrl], "http://127.0.0.1/cabin", "Cabin");
        let select_script = ScriptBuilder::new()
            .observe(&select_page)
            .observe(&select_page)
            .dom_input(30)
            .observe(&select_page)
            .dom_read_value(30, "Business", "Business")
            .observe(&select_page);
        std::fs::write(
            root.join("agent-select-cabin.cdp.json"),
            select_script.to_json(),
        )
        .unwrap();
    }
}
