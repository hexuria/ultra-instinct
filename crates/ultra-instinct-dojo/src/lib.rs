#![forbid(unsafe_code)]
//! The dojo: ultra-instinct's learning loop over its own battle diaries.
//!
//! The agent never touches files — it emits journal events; `ultra-instinct`
//! maps them into [`line::DiaryLine`]s and a [`diary::DiaryWriter`]. The
//! diary is the evidence base the replay arena, lesson store, and belt
//! exams all read. Lessons only ever adjust evidence inside the finite
//! `ActionSpace`; they never pick an off-menu action and never bypass the
//! guard/ticket/executor chain (the anti-drift rules own that path).

pub mod belt;
pub mod diary;
pub mod error;
pub mod exams;
pub mod lessons;
pub mod line;
pub mod rebuild;
pub mod site;
pub mod trust;

pub use belt::{label_belts, move_belt, Belt};
pub use diary::{read_diary, DiaryWriter};
pub use error::DojoError;
pub use exams::{examine, is_remote_source, ExamReport};
pub use lessons::{
    diary_id, diary_stamp_ms, learn_diary, learn_lines, load as load_lessons, save as save_lessons,
    LessonStore, Move, MoveStep, Place, Trust, Word, LESSON_SCHEMA,
};
pub use line::{
    parse_line, ChoiceLine, ClauseLine, CorrectionLine, DecisionLine, DiaryLine, HistoryLine,
    OfferedLine, OfferedState, OutcomeLine, RankedLine, RunLine, SiteLine, Situation, StaleLine,
    StepLine, SCHEMA,
};
pub use rebuild::{action_space, element_state};
pub use site::{context_key, site_line, situation_key};
pub use trust::{label_bonus_map, trust_bonus};
