#![no_main]

use std::io::{pipe, Write};

use libfuzzer_sys::fuzz_target;
use ultra_instinct_cdp::{leading_id, CdpTransport, Frame, FrameSplitter, PipeBrowser, PipeLimits};

// The CDP pipe reads NUL-delimited frames from a browser process. Two legs:
// the frame splitter under input-chosen chunking (oversize frames skipped,
// never buffered past the cap, the stream re-synced at the next NUL), and
// the whole pump (frame -> envelope decode -> id / session routing) on a real
// in-process pipe. Both must only return Err on malformed input; a panic or
// hang is a bug.
fuzz_target!(|data: &[u8]| {
    let Some((&step, stream)) = data.split_first() else {
        return;
    };
    let mut splitter = FrameSplitter::new(256);
    let chunk_len = usize::from(step).max(1);
    for chunk in stream.chunks(chunk_len) {
        splitter.push(chunk);
        while let Some(frame) = splitter.next_frame() {
            if let Frame::Message(frame) = frame {
                assert!(frame.len() <= 256 && !frame.contains(&0));
                let _ = leading_id(&frame);
            }
        }
        assert!(splitter.buffered() <= 256 + chunk_len);
    }

    // Stay well under the smallest pipe buffer so the write never blocks.
    let stream = &stream[..stream.len().min(8 * 1024)];
    let (browser_in, mut chrome_out) = pipe().expect("pipe");
    let (_chrome_in, browser_out) = pipe().expect("pipe");
    chrome_out.write_all(stream).expect("write");
    drop(chrome_out);
    let limits = PipeLimits {
        max_frame_bytes: 4096,
        ..PipeLimits::default()
    };
    let browser = PipeBrowser::with_limits(browser_in, browser_out, limits);
    if let Ok(mut page) = browser.page("T") {
        let _ = page.call("Runtime.evaluate", "{}");
        let _ = page.drain_events();
    }
    let _ = browser.call("Browser.getVersion", "{}");
    let _ = browser.drain_events();
});
