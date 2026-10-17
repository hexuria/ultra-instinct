//! Compact snapshot: one `Runtime.evaluate` collects the per-element
//! evidence `observe` otherwise gathers node by node (`DOM.getBoxModel`,
//! `CSS.getComputedStyleForNode`, `DOM.getNodeForLocation`).
//!
//! [`COMPACT_JS`] walks the same flattened tree `DOM.getDocument`
//! (`pierce: true`) exposes — element children, then open shadow roots,
//! then same-origin `contentDocument` — tags every element with a
//! `data-hu-k` attribute, and returns, per tag: the content-box rect in
//! top-viewport coordinates, the computed-style pairs
//! [`crate::stacking::style_from_computed`] reads, hit-test result
//! (`elementFromPoint`, recursing into same-origin iframes the way CDP hit
//! tests descend), and control state. `observe` then runs `DOM.getDocument`,
//! which sees the injected attributes and joins each record to its
//! `backendNodeId`.
//!
//! Elements the page cannot show to a same-origin walk — cross-origin
//! iframe content, closed shadow roots — carry no `data-hu-k`; `observe`
//! falls back to the per-node calls for exactly those. A replay script
//! that does not script the eval step falls back entirely, which keeps
//! every pre-compact fixture working unchanged.
//!
//! Semantics preserved from the per-node calls: `display:none` (no client
//! rects) omits a node like a `getBoxModel` error; the rect is the content
//! box (border box minus computed border + padding) in top-viewport
//! coordinates; hit tests start in the top document at top-viewport
//! coordinates (so parent overlays above an iframe count), descend into open
//! shadow roots, and descend into same-origin iframes, as
//! `DOM.getNodeForLocation` does. Style and hit evidence is collected only
//! for elements that can become regions; others fall back per node.
//!
//! Side effect: the `data-hu-k` attributes stay on the page and are visible
//! to its scripts. Each walk re-tags every reachable element; a key carried
//! by more than one element (a page clone, a forged value in an unreachable
//! frame) is dropped by `extract` and those elements fall back per node.
//! A walk that throws (hostile page globals) falls back entirely.

use std::collections::BTreeMap;

use serde_json::Value;

use aui_core::{ElementState, Rect};

use aui_cdp::CdpError;
pub(crate) use aui_cdp::HU_K_ATTR;

use crate::error::BrowserError;

/// The single-eval walk. Returns `{nodes: {"<k>": {r,s?,h?}}}` where `r` is
/// `[x,y,w,h]` or null, `s` the computed-style name/value pairs
/// `style_from_computed` consumes, and `h` the `data-hu-k` of the element
/// under the content center (null when nothing is there). `s`, `h`, and
/// control state (`v`, `c`, `x`, `sel`, `o`, `it`, `hr`) and the click hint `ck` are collected only for boxed
/// elements that can become regions (the JS mirror of `extract::keep_element`).
/// An absent `s` or `h` means "not collected" and the consumer falls back to
/// the per-node call.
pub(crate) const COMPACT_JS: &str = r#"(function(){
var ATTR='data-hu-k';
var PROPS=['z-index','position','opacity','transform','filter','isolation','mix-blend-mode','will-change','pointer-events'];
var nodes={};
var next=0;
function px(v){var n=parseFloat(v);return isFinite(n)?n:0;}
// Offset of a document's origin in top-viewport coordinates, via the
// frameElement chain (same-origin only).
function docOffset(doc){
  var x=0,y=0;
  try{
    var w=doc.defaultView;
    var f=w&&w!==w.top?w.frameElement:null;
    while(f){
      var r=f.getBoundingClientRect();
      var cs=f.ownerDocument.defaultView.getComputedStyle(f);
      x+=r.left+px(cs.borderLeftWidth)+px(cs.paddingLeft);
      y+=r.top+px(cs.borderTopWidth)+px(cs.paddingTop);
      var w2=f.ownerDocument.defaultView;
      f=w2&&w2!==w2.top?w2.frameElement:null;
    }
  }catch(e){}
  return[x,y];
}
// Content box in top-viewport coordinates, or null when the element has
// no client rects (display:none subtree) — the same omission a
// getBoxModel protocol error causes.
function contentRect(el,off){
  if(!el.getClientRects||el.getClientRects().length===0)return null;
  var r=el.getBoundingClientRect();
  var cs=el.ownerDocument.defaultView.getComputedStyle(el);
  var x=r.left+px(cs.borderLeftWidth)+px(cs.paddingLeft)+off[0];
  var y=r.top+px(cs.borderTopWidth)+px(cs.paddingTop)+off[1];
  var w=r.width-px(cs.borderLeftWidth)-px(cs.borderRightWidth)-px(cs.paddingLeft)-px(cs.paddingRight);
  var h=r.height-px(cs.borderTopWidth)-px(cs.borderBottomWidth)-px(cs.paddingTop)-px(cs.paddingBottom);
  if(!(w>0)||!(h>0))return null;
  return[x,y,w,h];
}
function stylePairs(el){
  var cs=el.ownerDocument.defaultView.getComputedStyle(el);
  var out=[];
  for(var i=0;i<PROPS.length;i++)out.push([PROPS[i],cs.getPropertyValue(PROPS[i])]);
  return out;
}
// Nearest tagged ancestor-or-self of a hit node, climbing out of shadow
// roots and (same-origin) document boundaries.
function hitKey(node){
  var n=node;
  while(n){
    if(n.nodeType===1&&n.hasAttribute&&n.hasAttribute(ATTR))return parseInt(n.getAttribute(ATTR),10);
    if(n.parentNode){n=n.parentNode;continue;}
    if(n.host){n=n.host;continue;}
    break;
  }
  return null;
}
// Deepest element under a point in `root`'s coordinates, descending into
// open shadow roots: `elementFromPoint` retargets to the shadow host, while
// CDP hit testing returns the node inside the shadow tree.
function deepAt(root,x,y){
  var el=null;
  try{el=root.elementFromPoint(x,y);}catch(e){return null;}
  while(el&&el.shadowRoot){
    var inner=null;
    try{inner=el.shadowRoot.elementFromPoint(x,y);}catch(e){}
    if(!inner||inner===el)break;
    el=inner;
  }
  return el;
}
// Hit test at top-viewport point (tx,ty), starting in `doc` (always the top
// document from `visit`, so parent overlays above an iframe are seen) and
// descending into same-origin iframe hits the way CDP hit testing does.
function hitAt(doc,tx,ty){
  var o=docOffset(doc);
  var el=deepAt(doc,tx-o[0],ty-o[1]);
  if(!el)return null;
  try{
    if(el.contentDocument){
      var inner=hitAt(el.contentDocument,tx,ty);
      if(inner!==null)return inner;
    }
  }catch(e){}
  return hitKey(el);
}
// Mirrors `extract::keep_element`: only elements that can become regions
// get style and hit evidence. Everything else carries a rect only; a region
// whose element lacks evidence takes the per-node fallback.
var KEEP={BUTTON:1,A:1,INPUT:1,TEXTAREA:1,SELECT:1,OPTION:1,NAV:1,IMG:1,H1:1,H2:1,H3:1,H4:1,H5:1,H6:1,DIALOG:1};
var TEXTUAL={LABEL:1,SPAN:1,P:1,DIV:1};
function directText(el){
  for(var c=el.firstChild;c;c=c.nextSibling){
    if(c.nodeType===3&&c.nodeValue&&c.nodeValue.trim()!=='')return true;
  }
  return false;
}
// A script-made click target: an `onclick` handler property, or a pointer
// cursor the element sets itself (not one inherited from a clickable
// ancestor, which already owns the click).
function clickHint(el){
  if(typeof el.onclick==='function')return true;
  var view=el.ownerDocument.defaultView;
  if(view.getComputedStyle(el).cursor!=='pointer')return false;
  var p=el.parentElement;
  return !p||view.getComputedStyle(p).cursor!=='pointer';
}
var SVG_NS='http://www.w3.org/2000/svg';
var SHAPES={circle:1,ellipse:1,rect:1,polygon:1,polyline:1,path:1,text:1};
// A box whose own content scrolls: overflow-y auto / scroll with more
// content than room.
function scrollable(el){
  var oy=el.ownerDocument.defaultView.getComputedStyle(el).overflowY;
  return (oy==='auto'||oy==='scroll')&&el.scrollHeight>el.clientHeight+4;
}
function candidate(el){
  var name=el.nodeName.toUpperCase();
  if(name==='SCRIPT'||name==='STYLE'||name==='HEAD'||name==='HTML'||name==='BODY')return false;
  if(el.namespaceURI===SVG_NS&&SHAPES[el.localName])return true;
  if(el.hasAttribute('role'))return true;
  if(KEEP[name])return true;
  if(!TEXTUAL[name])return false;
  var aria=el.getAttribute('aria-label');
  // A text-less box counts only with a click hint (extract::needs_hint).
  return (aria!==null&&aria.trim()!=='')||directText(el)||clickHint(el)||scrollable(el);
}
function visit(el,doc,off){
  if(el.nodeType!==1)return;
  var k=next++;
  el.setAttribute(ATTR,String(k));
  var rec={r:null};
  var r=contentRect(el,off);
  if(r){
    rec.r=r;
    if(candidate(el)){
      rec.s=stylePairs(el);
      rec.h=hitAt(document,r[0]+r[2]/2,r[1]+r[3]/2);
      if(clickHint(el))rec.ck=1;
      if(scrollable(el))rec.sc=1;
      var name=el.nodeName.toUpperCase();
      var inputType=(el.getAttribute('type')||'text').toLowerCase();
      if(name==='INPUT')rec.it=inputType;
      // A destination only for real navigations: not `javascript:`, not `#`/`` back to this page.
      if(name==='A'&&el.href&&/^https?:$/.test(el.protocol)&&!(el.hash===''&&el.href.split('#')[0]===location.href.split('#')[0])){
        rec.hr=Array.from(String(el.href)).slice(0,500).join('');
      }
      if((name==='INPUT'&&inputType!=='password'&&inputType!=='hidden'&&inputType!=='file'&&inputType!=='checkbox'&&inputType!=='radio')||name==='TEXTAREA'){
        rec.v=Array.from(String(el.value)).slice(0,200).join('');
      }
      if(name==='INPUT'&&(inputType==='checkbox'||inputType==='radio')){
        rec.c=!!el.checked;
      }else{
        var checked=el.getAttribute('aria-checked');
        if(checked==='true'||checked==='false')rec.c=checked==='true';
      }
      var expanded=el.getAttribute('aria-expanded');
      if(expanded==='true'||expanded==='false')rec.x=expanded==='true';
      if(name==='SELECT'){
        var selected=[];
        var options=[];
        for(var j=0;j<el.options.length;j++){
          var option=el.options[j];
          var label=(option.label||option.textContent||'').trim();
          if(option.selected)selected.push(label);
          var group=option.parentElement;
          var disabledGroup=group&&group.nodeName.toUpperCase()==='OPTGROUP'&&group.disabled;
          if(!option.disabled&&!disabledGroup&&options.length<50){
            options.push(Array.from(label).slice(0,80).join(''));
          }
        }
        rec.sel=selected.join(', ');
        rec.o=options;
      }
    }
  }
  nodes[k]=rec;
  var kids=el.children;
  if(kids)for(var i=0;i<kids.length;i++)visit(kids[i],doc,off);
  if(el.shadowRoot){
    var shadow=el.shadowRoot.children;
    if(shadow)for(var i=0;i<shadow.length;i++)visit(shadow[i],doc,off);
  }
  try{
    var cd=el.contentDocument;
    if(cd&&cd.children){
      var off2=docOffset(cd);
      for(var i=0;i<cd.children.length;i++)visit(cd.children[i],cd,off2);
    }
  }catch(e){}
}
(function(){
  var off=docOffset(document);
  var top=document.children;
  for(var i=0;i<top.length;i++)visit(top[i],document,off);
})();
return{nodes:nodes};
})()"#;

/// Per-element evidence from one [`COMPACT_JS`] run.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompactNode {
    /// Content box in top-viewport coordinates; `None` = omit like a
    /// `getBoxModel` protocol error.
    pub rect: Option<Rect>,
    /// `CSS.getComputedStyleForNode` name/value pairs; `None` = not
    /// collected (per-node fallback).
    pub style: Option<Vec<(String, String)>>,
    /// Hit test at the content center.
    pub hit: Hit,
    /// Control state captured by the compact walk.
    pub state: ElementState,
    /// The page made this element clickable by script: an `onclick`
    /// property, or a pointer cursor it sets itself.
    pub click_hint: bool,
    /// The element's own content scrolls (overflow-y auto / scroll, taller
    /// than its box).
    pub scrollable: bool,
}

/// Compact hit-test evidence for one element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Hit {
    /// The walk did not hit-test this element; use `DOM.getNodeForLocation`.
    #[default]
    NotCollected,
    /// Nothing tagged lies under the point (like a protocol error: skip).
    Nothing,
    /// `data-hu-k` of the element under the point.
    Key(u32),
}

/// One compact snapshot keyed by `data-hu-k`.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompactSnapshot {
    pub nodes: BTreeMap<u32, CompactNode>,
}

impl CompactSnapshot {
    pub fn node(&self, k: u32) -> Option<&CompactNode> {
        self.nodes.get(&k)
    }
}

/// Trust-boundary entry for the `aui-fuzz` `compact_snapshot` target.
/// The JSON is CDP-controlled in production but a live page shapes the node
/// records inside it, so the parser must never panic. Returns `true` when the
/// blob parses; every malformed input must return `false`, not panic.
#[cfg(feature = "fuzz-support")]
pub fn compact_parse_fuzzable(eval_result_json: &str) -> bool {
    parse(eval_result_json).is_ok()
}

/// Parse the `Runtime.evaluate` result of [`COMPACT_JS`].
pub(crate) fn parse(eval_result_json: &str) -> Result<CompactSnapshot, BrowserError> {
    let value: Value = serde_json::from_str(eval_result_json).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    let nodes = value
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("nodes"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            BrowserError::Cdp(CdpError::BadJson {
                message: "compact snapshot result has no nodes object".into(),
            })
        })?;
    let mut out = CompactSnapshot::default();
    for (k, record) in nodes {
        let k: u32 = k.parse().map_err(|_| CdpError::BadJson {
            message: format!("compact node key `{k}` is not a u32"),
        })?;
        let rect = record.get("r").and_then(Value::as_array).and_then(|r| {
            if r.len() < 4 {
                return None;
            }
            let x = r[0].as_f64()?;
            let y = r[1].as_f64()?;
            let w = r[2].as_f64()?;
            let h = r[3].as_f64()?;
            Rect::try_new(x, y, w, h).ok()
        });
        let style = record.get("s").and_then(Value::as_array).map(|pairs| {
            pairs
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    let name = pair.first()?.as_str()?;
                    let value = pair.get(1)?.as_str()?;
                    Some((name.to_owned(), value.to_owned()))
                })
                .collect()
        });
        let hit = match record.get("h") {
            None => Hit::NotCollected,
            Some(Value::Null) => Hit::Nothing,
            Some(value) => {
                let key = value
                    .as_u64()
                    .and_then(|h| u32::try_from(h).ok())
                    .ok_or_else(|| CdpError::BadJson {
                        message: format!("compact node `{k}` hit `{value}` is not a u32"),
                    })?;
                Hit::Key(key)
            }
        };
        let state = parse_element_state(record);
        let click_hint = record.get("ck").is_some_and(|ck| ck.as_i64() == Some(1));
        let scrollable = record.get("sc").is_some_and(|sc| sc.as_i64() == Some(1));
        out.nodes.insert(
            k,
            CompactNode {
                rect,
                style,
                hit,
                state,
                click_hint,
                scrollable,
            },
        );
    }
    Ok(out)
}

fn parse_element_state(record: &Value) -> ElementState {
    let options = record
        .get("o")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(Value::as_str)
                .take(50)
                .map(|value| truncate_chars(value, 80))
                .collect()
        })
        .unwrap_or_default();
    ElementState {
        value: record
            .get("v")
            .and_then(Value::as_str)
            .map(|value| truncate_chars(value, 200)),
        checked: record.get("c").and_then(Value::as_bool),
        expanded: record.get("x").and_then(Value::as_bool),
        selected: record.get("sel").and_then(Value::as_str).map(str::to_owned),
        options,
        input_type: record.get("it").and_then(Value::as_str).map(str::to_owned),
        href: record
            .get("hr")
            .and_then(Value::as_str)
            .map(|href| truncate_chars(href, 500)),
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_state(record: Value) -> Result<ElementState, BrowserError> {
        let input = json!({"result":{"value":{"nodes":{"1":record}}}}).to_string();
        Ok(parse(&input)?.node(1).unwrap().state.clone())
    }

    #[test]
    fn click_hint_is_set_only_by_ck_one() {
        let hint = |record: Value| {
            let input = json!({"result":{"value":{"nodes":{"1":record}}}}).to_string();
            parse(&input).unwrap().node(1).unwrap().click_hint
        };
        assert!(hint(json!({"r":[0,0,10,10],"ck":1})));
        assert!(!hint(json!({"r":[0,0,10,10]})));
        assert!(!hint(json!({"r":[0,0,10,10],"ck":"1"})));
        assert!(!hint(json!({"r":[0,0,10,10],"ck":0})));
    }

    #[test]
    fn compact_control_state_parses_all_fields() {
        assert_eq!(
            parse_state(json!({
                "v":"Ana",
                "c":true,
                "x":false,
                "sel":"UTC, Asia/Manila",
                "o":["UTC", "Asia/Manila"],
                "hr":"https://example.com/a"
            }))
            .unwrap(),
            ElementState {
                value: Some("Ana".to_owned()),
                checked: Some(true),
                expanded: Some(false),
                selected: Some("UTC, Asia/Manila".to_owned()),
                options: vec!["UTC".to_owned(), "Asia/Manila".to_owned()],
                input_type: None,
                href: Some("https://example.com/a".to_owned()),
            }
        );
    }

    #[test]
    fn wrong_typed_control_state_fields_are_ignored() {
        assert_eq!(
            parse_state(json!({"v":5,"c":"yes","o":"x","sel":[1]})).unwrap(),
            ElementState::default()
        );
    }

    #[test]
    fn compact_value_is_truncated_by_characters() {
        let value = "é".repeat(300);
        let state = parse_state(json!({"v":value})).unwrap();
        assert_eq!(state.value.unwrap().chars().count(), 200);
    }

    #[test]
    fn compact_options_are_limited_by_characters_and_count() {
        let options: Vec<_> = (0..55).map(|_| "界".repeat(100)).collect();
        let state = parse_state(json!({"o":options})).unwrap();
        assert_eq!(state.options.len(), 50);
        assert!(state
            .options
            .iter()
            .all(|option| option.chars().count() == 80));
    }
}
