//! Text rendering of the `daemon` commands' responses.

use std::io::{self, Write};

use codix_core::render::{Style, TextRender, elapsed};

use crate::control::{ListResponse, StartResponse, StopResponse};

impl TextRender for StartResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        if let Some(old) = &self.replaced {
            writeln!(
                w,
                "stopped the codix {} daemon (pid {}), which this codix cannot talk to",
                old.version, old.pid
            )?;
        }
        let d = &self.daemon;
        writeln!(
            w,
            "daemon started for {}: pid {}, socket {}",
            d.root.display(),
            d.pid,
            d.socket
        )
    }
}

impl TextRender for StopResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        if self.stopped.is_empty() {
            return writeln!(w, "no daemon running");
        }
        for d in &self.stopped {
            writeln!(w, "stopped daemon for {} (pid {})", d.root.display(), d.pid)?;
        }
        Ok(())
    }
}

impl TextRender for ListResponse {
    fn render_text(&self, w: &mut dyn Write, _: Style) -> io::Result<()> {
        if self.daemons.is_empty() {
            return writeln!(w, "no daemon running");
        }
        for listed in &self.daemons {
            let d = &listed.daemon;
            let stats = listed.stats.as_ref().map_or_else(String::new, |s| {
                format!(
                    "  {} MB  idle {}",
                    s.memory_bytes >> 20,
                    elapsed(s.idle_seconds)
                )
            });
            writeln!(
                w,
                "{}  pid {}{stats}  codix {}",
                d.root.display(),
                d.pid,
                d.version
            )?;
        }
        writeln!(w, "{} running", self.daemons.len())
    }
}
