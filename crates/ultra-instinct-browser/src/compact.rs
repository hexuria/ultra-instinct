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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;

use ultra_instinct_core::{ElementState, FormSubmit, Rect};

use ultra_instinct_cdp::CdpError;
pub(crate) use ultra_instinct_cdp::HU_K_ATTR;

use crate::error::BrowserError;

/// The single-eval walk. Returns `{nodes: {"<k>": {r,s?,h?}}}` where `r` is
/// `[x,y,w,h]` or null, `s` the computed-style name/value pairs
/// `style_from_computed` consumes, and `h` the `data-hu-k` of the element
/// under the content center (null when nothing is there). `s`, `h`, and
/// control state (`v`, `c`, `x`, `sel`, `o`, `it`, `hr`) and the click hint `ck` are collected only for boxed
/// elements that can become regions: controls, roles, surfaces, shapes,
/// drag-marked elements, and every plain box (`extract::is_box`), since a
/// box's click listener is read after the walk.
/// An absent `s` or `h` means "not collected" and the consumer falls back to
/// the per-node call.
pub(crate) const COMPACT_JS: &str = r#"(function(){
var ATTR='data-hu-k';
var PROPS=['z-index','position','opacity','transform','filter','isolation','mix-blend-mode','will-change','pointer-events'];
var nodes={};
var next=0;
// `fm` on a form control is its form's index in `forms`, each entry
// [button, action]: the implicit-submission button (its first submit button
// in tree order) by accessible name (aria-labelledby, aria-label, text /
// value / alt / title, as the AX tree names it), or with no submit button 2
// when Enter still submits it (at most one field blocks implicit
// submission), else 1; and where that sends the form. No `fm`: Enter there
// submits no form. `fa` on a submit button: where pressing it sends it.
var FORMS=new Map(),forms=[];
var BLOCKS={text:1,search:1,url:1,tel:1,email:1,password:1,date:1,month:1,week:1,time:1,'datetime-local':1,number:1};
function fold(t){return String(t||'').replace(/\s+/g,' ').trim();}
function accName(b){
  var root=b.getRootNode?b.getRootNode():b.ownerDocument;
  var by=fold(b.getAttribute('aria-labelledby')).split(' ').map(function(id){
    var t=id&&(root.getElementById?root:b.ownerDocument).getElementById(id);
    return t?fold(t.innerText||t.textContent):'';
  }).filter(Boolean).join(' ');
  if(by)return by;
  var label=fold(b.getAttribute('aria-label'));
  if(label)return label;
  if(b.nodeName!=='INPUT')return fold(b.innerText||b.textContent)||fold(b.getAttribute('title'));
  var image=String(b.type).toLowerCase()==='image';
  return fold(image?(b.getAttribute('alt')||b.value):b.value)||fold(b.getAttribute('title'))||(image?'':'Submit');
}
// The resolved formaction / action ('' action: the page); '' when unreadable.
function act(f,b){
  try{var a=(b&&b.getAttribute('formaction'))||f.getAttribute('action');return(a?new URL(a,f.ownerDocument.baseURI).href:f.ownerDocument.URL).slice(0,500);}catch(e){return '';}
}
function formSubmit(f){
  if(!FORMS.has(f)){
    var b=null,blocking=0,els=f.elements||[];
    for(var i=0;i<els.length;i++){
      var e=els[i],type=String(e.type).toLowerCase();
      if(!b&&(e.nodeName==='BUTTON'||e.nodeName==='INPUT')&&type==='submit')b=e;
      if(e.nodeName==='INPUT'&&BLOCKS[type])blocking++;
    }
    var img=f.querySelector('input[type=image]');
    if(img&&(!b||(img.compareDocumentPosition(b)&4)))b=img;
    FORMS.set(f,forms.length);
    forms.push([b?Array.from(accName(b)).slice(0,200).join(''):(blocking<=1?2:1),act(f,b)]);
  }
  return FORMS.get(f);
}
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
// Mirrors `extract::is_box`.
var BOXES={LABEL:1,SPAN:1,P:1,DIV:1,LI:1,TD:1,TH:1,TR:1,I:1,B:1,EM:1,STRONG:1,SMALL:1,FIGURE:1,SECTION:1,ARTICLE:1};
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
// Mirrors `extract::drag_markup`.
var DRAG_MARKERS=['ui-draggable','ui-sortable-handle','ui-slider-handle','ui-resizable-handle'];
function dragMarked(el){
  if((el.getAttribute('draggable')||'').toLowerCase()==='true')return true;
  if(el.hasAttribute('aria-grabbed'))return true;
  var cl=el.classList;
  if(cl)for(var i=0;i<DRAG_MARKERS.length;i++)if(cl.contains(DRAG_MARKERS[i]))return true;
  return false;
}
// A move / grab cursor the element sets itself: one inherited from a
// draggable ancestor belongs to that ancestor (as in `clickHint`).
function dragCursor(el){
  var view=el.ownerDocument.defaultView;
  var grabs=function(c){return c==='move'||c==='grab'||c==='grabbing';};
  var cur=view.getComputedStyle(el).cursor;
  if(!grabs(cur))return false;
  var p=el.parentElement;
  return !p||view.getComputedStyle(p).cursor!==cur;
}
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
  if(name==='CANVAS'||(el.namespaceURI===SVG_NS&&el.localName==='svg'))return true;
  if(dragMarked(el))return true;
  if(el.hasAttribute('role'))return true;
  if(KEEP[name])return true;
  // Every box: whether Chrome reports a click listener on it is known only
  // after this walk (DOMSnapshot), so each carries its evidence.
  return !!BOXES[name];
}
function visit(el,doc,off){
  if(el.nodeType!==1)return;
  var k=next++;
  el.setAttribute(ATTR,String(k));
  var rec={r:null},tag=el.nodeName.toUpperCase();
  // Any geometry. Enter in a text area or a select submits no form.
  if((tag==='INPUT'||tag==='BUTTON')&&el.form){rec.fm=formSubmit(el.form);if(/^(submit|image)$/i.test(el.type))rec.fa=act(el.form,el);}
  // Not rendered at all (display:none here or above): its text names nothing.
  if(el.getClientRects&&el.getClientRects().length===0)rec.nr=1;
  var r=contentRect(el,off);
  if(r){
    rec.r=r;
    if(candidate(el)){
      rec.s=stylePairs(el);
      rec.h=hitAt(document,r[0]+r[2]/2,r[1]+r[3]/2);
      if(clickHint(el))rec.ck=1;
      if(scrollable(el))rec.sc=1;
      if(dragCursor(el))rec.dg=1;
      // Paint, for targets named by colour alone.
      var paint=el.ownerDocument.defaultView.getComputedStyle(el);
      var bgc=paint.backgroundColor;
      if(bgc&&bgc!=='transparent'&&bgc!=='rgba(0, 0, 0, 0)')rec.bg=bgc;
      if(el.namespaceURI===SVG_NS&&SHAPES[el.localName]&&paint.fill&&paint.fill!=='none')rec.fl=paint.fill;
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
      // A label stands for its checkbox or radio: its state is the control's.
      var ctl=name==='LABEL'?el.control:null;
      if(name==='INPUT'&&(inputType==='checkbox'||inputType==='radio')){
        rec.c=!!el.checked;
      }else if(ctl&&(ctl.type==='checkbox'||ctl.type==='radio')){
        rec.c=!!ctl.checked;
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
return{nodes:nodes,forms:forms};
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
    /// The element sets a move / grab cursor itself (not inherited from its
    /// parent): it can be dragged.
    pub drag_hint: bool,
    /// Not rendered at all: no client rects (`display: none`).
    pub unrendered: bool,
    /// Computed `background-color`, when it is not transparent.
    pub background: Option<String>,
    /// Computed `fill` of an SVG shape, when it is not `none`.
    pub fill: Option<String>,
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

    /// Keys of elements that are not rendered at all (`display: none` on
    /// them or an ancestor). A zero-size box is still rendered.
    pub fn unrendered(&self) -> BTreeSet<u32> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.unrendered)
            .map(|(k, _)| *k)
            .collect()
    }
}

/// Trust-boundary entry for the `ultra-instinct-fuzz` `compact_snapshot` target.
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
    // The per-form table `fm` indexes, one allocation per form: [a label,
    // or 1 / 2 for no submit button that Enter does not / does submit; where
    // it is sent ('' unreadable)].
    let forms: Vec<Option<FormSubmit>> = value["result"]["value"]["forms"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|form| {
            let action = Some(shared_url(&form[1])).filter(|a| !a.is_empty());
            match &form[0] {
                Value::String(label) => Some(FormSubmit::Button {
                    label: truncate_chars(label, 200).into(),
                    action,
                }),
                Value::Number(n) if n.as_u64() == Some(1) => Some(FormSubmit::NoSubmitButton),
                Value::Number(n) if n.as_u64() == Some(2) => Some(FormSubmit::Implicit { action }),
                _ => None,
            }
        })
        .collect();
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
        let state = parse_element_state(record, &forms);
        let click_hint = record.get("ck").is_some_and(|ck| ck.as_i64() == Some(1));
        let scrollable = record.get("sc").is_some_and(|sc| sc.as_i64() == Some(1));
        let drag_hint = record.get("dg").is_some_and(|dg| dg.as_i64() == Some(1));
        let unrendered = record.get("nr").is_some_and(|nr| nr.as_i64() == Some(1));
        let paint = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .map(|v| truncate_chars(v, 64))
        };
        let background = paint("bg");
        let fill = paint("fl");
        out.nodes.insert(
            k,
            CompactNode {
                rect,
                style,
                hit,
                state,
                click_hint,
                scrollable,
                drag_hint,
                unrendered,
                background,
                fill,
            },
        );
    }
    Ok(out)
}

/// A form target URL, cut to 500 chars; `""` when it is not a string.
fn shared_url(value: &Value) -> Arc<str> {
    value
        .as_str()
        .map_or("", |t| t)
        .chars()
        .take(500)
        .collect::<String>()
        .into()
}

fn parse_element_state(record: &Value, forms: &[Option<FormSubmit>]) -> ElementState {
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
        // No `fm`: Enter there submits no form; an index the table lacks
        // is unread evidence.
        // A submit button's target: `''` / unread ⇒ `Some("")`.
        submit_action: record.get("fa").map(shared_url),
        form: match record.get("fm") {
            None => Some(FormSubmit::NotInForm),
            Some(index) => index
                .as_u64()
                .and_then(|i| forms.get(usize::try_from(i).ok()?).cloned().flatten()),
        },
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
        let forms = json!([
            ["Pay $18", "https://shop.example/pay"],
            [1, ""],
            [2, ""],
            ["", "https://shop.example/"],
            [true, ""]
        ]);
        let input = json!({"result":{"value":{"nodes":{"1":record},"forms":forms}}}).to_string();
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
                "hr":"https://example.com/a",
                "fm":0,
                "fa":"https://shop.example/pay"
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
                form: Some(FormSubmit::Button {
                    label: "Pay $18".into(),
                    action: Some("https://shop.example/pay".into()),
                }),
                submit_action: Some("https://shop.example/pay".into()),
            }
        );
        // `fm` indexes the per-form table: [a label (an empty one too), or
        // 1 / 2 for no submit button that Enter does not / does submit; the
        // target, '' unread]; no `fm`: no form; an index past the table or a
        // bad entry: unread. `fa` that is no string: an unread target.
        let form = |record: Value| parse_state(record).unwrap().form;
        assert_eq!(form(json!({})), Some(FormSubmit::NotInForm));
        assert_eq!(form(json!({"fm": 1})), Some(FormSubmit::NoSubmitButton));
        assert_eq!(
            form(json!({"fm": 2})),
            Some(FormSubmit::Implicit { action: None })
        );
        assert_eq!(
            form(json!({"fm": 3})),
            Some(FormSubmit::Button {
                label: "".into(),
                action: Some("https://shop.example/".into()),
            })
        );
        assert_eq!(form(json!({"fm": 4})), None);
        assert_eq!(form(json!({"fm": 9})), None);
        assert_eq!(form(json!({"fm": "Pay $18"})), None);
        let fa = |record: Value| parse_state(record).unwrap().submit_action;
        assert_eq!(fa(json!({})), None);
        assert_eq!(fa(json!({"fa": null})), Some("".into()));
        // One form's strings are one allocation shared by its controls.
        let snapshot = parse(
            &json!({"result":{"value":{"nodes":{"1":{"fm":0},"2":{"fm":0}},
                "forms":[["Pay $18", "https://shop.example/pay"]]}}})
            .to_string(),
        )
        .unwrap();
        let label = |k| match &snapshot.node(k).unwrap().state.form {
            Some(FormSubmit::Button { label, .. }) => label.clone(),
            other => panic!("{other:?}"),
        };
        assert!(Arc::ptr_eq(&label(1), &label(2)));
    }

    #[test]
    fn wrong_typed_control_state_fields_are_ignored() {
        assert_eq!(
            parse_state(json!({"v":5,"c":"yes","o":"x","sel":[1]})).unwrap(),
            ElementState {
                form: Some(FormSubmit::NotInForm),
                ..ElementState::default()
            }
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
