//! R0 exit-criterion smoke binary (plan §7 R0): bring up compositor +
//! headless backend + noop renderer through the fence crate, run the
//! event loop, exit 0 on SIGTERM — mirroring the Phase-1 C smoke test.
//!
//! Throwaway by design: the real frontend (`westonite` crate) replaces
//! it at R2.  It deliberately uses only the safe public API.

use weston::{CompositorBuilder, Event, EventSink, ShellHost};

struct Smoke;

impl EventSink for Smoke {
    fn on_event(&mut self, _ctx: &weston::Ctx, event: Event) {
        eprintln!("westonite-r0: event {event:?}");
    }
}

fn main() -> std::process::ExitCode {
    let compositor = match CompositorBuilder::headless()
        .renderer(weston::RendererKind::Noop)
        .output_size(1024, 768)
        .with_socket()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("westonite-r0: startup failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    compositor.ctx().set_sink(Box::new(Smoke));

    // Prove the safe query surface works: enumerate outputs.
    let ctx = compositor.ctx().clone();
    for id in ctx.outputs() {
        match ctx.output_info(id) {
            Some(info) => eprintln!(
                "westonite-r0: output {:?} \"{}\" {}x{}",
                id, info.name, info.geometry.width, info.geometry.height
            ),
            None => eprintln!("westonite-r0: output {id:?} (stale)"),
        }
    }

    let mut compositor = compositor;
    let code = compositor.run();
    drop(compositor);
    eprintln!("westonite-r0: clean exit ({code})");
    std::process::ExitCode::from(code as u8)
}
