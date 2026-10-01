//! Native raster regression: presented preserves a stale black box; next paints a red flower.
//! Run `mise exec -- cargo run --example capture_fidelity` on Windows.
#[cfg(windows)]
mod fixture {
    use gpui::{
        App, Application, Bounds, Context, Render, Timer, Window, WindowBounds, WindowOptions, div,
        prelude::*, px, size,
    };
    use std::{cell::Cell, rc::Rc, sync::Arc, time::Duration};

    struct Glyph {
        intended_flower: Rc<Cell<bool>>,
        painted: Rc<Cell<bool>>,
    }
    impl Render for Glyph {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.painted.set(self.intended_flower.get());
            div()
                .size_full()
                .bg(gpui::white())
                .text_size(px(80.))
                .text_color(if self.intended_flower.get() {
                    gpui::rgb(0xff0000)
                } else {
                    gpui::rgb(0x000000)
                })
                .child(if self.intended_flower.get() {
                    "❀"
                } else {
                    "■"
                })
        }
    }
    fn export(gpu: gpui::GpuCapture, path: std::path::PathBuf) -> image::RgbaImage {
        let dimensions = (gpu.width, gpu.height);
        let image =
            image::RgbaImage::from_raw(dimensions.0, dimensions.1, gpu.read().unwrap()).unwrap();
        image.save(path).unwrap();
        image
    }
    pub fn run() {
        Application::new().run(|cx: &mut App| {
            let painted = Rc::new(Cell::new(false));
            let marker = painted.clone();
            let intended = Rc::new(Cell::new(false));
            let model = intended.clone();
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(320.), px(160.)),
                            cx,
                        ))),
                        ..Default::default()
                    },
                    move |_, cx| {
                        cx.new(|_| Glyph {
                            intended_flower: model,
                            painted: marker,
                        })
                    },
                )
                .unwrap();
            let handle = gpui::AnyWindowHandle::from(handle);
            handle
                .update(cx, |_, window, _| {
                    let scene_revision = Cell::new(0_u64);
                    window.on_frame_prepared(move |_, _| {
                        scene_revision.set(scene_revision.get() + 1);
                        Arc::new((scene_revision.get(), painted.get()))
                    });
                    window.refresh();
                })
                .unwrap();
            cx.spawn(async move |cx| {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while handle
                    .update(cx, |_, window, _| window.presented_metadata())
                    .unwrap()
                    .is_none()
                {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "initial presentation unavailable"
                    );
                    Timer::after(Duration::from_millis(20)).await;
                }
                // Deliberately omit notify: current model changes, displayed pixels stay stale.
                intended.set(true);
                let (old, metadata) = handle
                    .update(cx, |_, window, _| {
                        let before = window.presented_frame_id();
                        let frame = window.capture_presented().unwrap();
                        assert_eq!(
                            before,
                            window.presented_frame_id(),
                            "presented acquisition painted"
                        );
                        frame
                    })
                    .unwrap();
                assert!(!metadata.unwrap().downcast::<(u64, bool)>().unwrap().1);
                let old_id = old.frame_id;
                let directory = std::env::temp_dir().join(format!(
                    "mightty-raster-{}",
                    mightty::feedback::unix_timestamp_ms()
                ));
                std::fs::create_dir(&directory).unwrap();
                let path = directory.join("presented-black-box.png");
                let before = cx
                    .background_executor()
                    .spawn(async move { export(old, path) })
                    .await;
                let (again, metadata) = handle
                    .update(cx, |_, window, _| window.capture_presented().unwrap())
                    .unwrap();
                assert!(again.frame_id >= old_id);
                assert!(!metadata.unwrap().downcast::<(u64, bool)>().unwrap().1);
                let bytes = cx
                    .background_executor()
                    .spawn(async move { again.read().unwrap() })
                    .await;
                assert_eq!(
                    bytes,
                    before.as_raw().as_slice(),
                    "retained pixels changed with current model"
                );
                let baseline = handle
                    .update(cx, |_, window, _| {
                        let baseline = window
                            .prepared_metadata()
                            .unwrap()
                            .downcast::<(u64, bool)>()
                            .unwrap()
                            .0;
                        window.refresh();
                        baseline
                    })
                    .unwrap();
                while handle
                    .update(cx, |_, window, _| {
                        window
                            .presented_metadata()
                            .unwrap()
                            .downcast::<(u64, bool)>()
                            .unwrap()
                            .0
                    })
                    .unwrap()
                    <= baseline
                {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "next presentation unavailable"
                    );
                    Timer::after(Duration::from_millis(20)).await;
                }
                let (next, metadata) = handle
                    .update(cx, |_, window, _| window.capture_presented().unwrap())
                    .unwrap();
                assert!(metadata.unwrap().downcast::<(u64, bool)>().unwrap().1);
                let path = directory.join("next-red-flower.png");
                let after = cx
                    .background_executor()
                    .spawn(async move { export(next, path) })
                    .await;
                let red = |image: &image::RgbaImage| {
                    image
                        .pixels()
                        .filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80)
                        .count()
                };
                assert_eq!(red(&before), 0);
                assert!(red(&after) > 20, "flower color missing from raster output");
                assert_ne!(before.as_raw(), after.as_raw());
                println!("Raster fidelity passed; artifacts: {}", directory.display());
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    }
}
#[cfg(windows)]
fn main() {
    fixture::run();
}
#[cfg(not(windows))]
fn main() {
    println!("This native raster fixture requires Windows.");
}
