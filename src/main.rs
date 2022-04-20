use tokio::{self};

use ashpd::{
    desktop::screencast::{CursorMode, PersistMode, ScreenCastProxy, SourceType},
    enumflags2::BitFlags,
    WindowIdentifier,
};

use gstreamer::prelude::*;

use gstreamer_video::{is_video_overlay_prepare_window_handle_message, prelude::*};

use gstreamer as gst;

use winit::{
    dpi::{LogicalPosition, LogicalSize, Position, Size},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    platform::unix::{WindowBuilderExtUnix, WindowExtUnix},
    window::WindowBuilder,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::Connection::session().await?;
    let proxy = ScreenCastProxy::new(&connection).await?;

    let session = proxy.create_session().await?;
    let identifier = WindowIdentifier::None;

    proxy
        .select_sources(
            &session,
            BitFlags::from(CursorMode::Embedded),
            BitFlags::from(SourceType::Monitor),
            false,
            None,
            PersistMode::ExplicitlyRevoked,
        )
        .await?;

    let (streams, _) = proxy.start(&session, &identifier).await?;

    let fd = proxy.open_pipe_wire_remote(&session).await?;

    let node = streams
        .get(0)
        .map(|stream| stream.pipe_wire_node_id())
        .expect("Failed to load node!");

    // Initialize GStreamer
    gst::init().unwrap();

    // Create the elements
    let source = gst::ElementFactory::make("pipewiresrc", Some("source"))
        .expect("Could not create source element.");

    let sink = gst::ElementFactory::make("ximagesink", Some("sink"))
        .expect("Could not create sink element");

    let first_queue =
        gst::ElementFactory::make("queue", Some("first-queue")).expect("Failed to create queue");
    let second_queue = gst::ElementFactory::make("videoscale", Some("second-queue"))
        .expect("Failed to create queue");
    let convert = gst::ElementFactory::make("videoconvert", None).expect("Failed to create queue");

    // Create the empty pipeline
    let pipeline = gst::Pipeline::new(Some("test-pipeline"));

    // Modify the source's properties
    source.set_property("fd", &fd);
    source.set_property("path", &node.to_string());

    // Build the pipeline
    pipeline
        .add_many(&[&source, &first_queue, &convert, &second_queue, &sink])
        .unwrap();

    source
        .link(&first_queue)
        .expect("Elements could not be linked.");

    first_queue
        .link(&convert)
        .expect("Elements could not be linked.");

    convert
        .link(&second_queue)
        .expect("Elements could not be linked.");
    second_queue
        .link(&sink)
        .expect("Elements could not be linked.");

    let event_loop = EventLoop::new();

    let title = "cast-to-x";

    let window = WindowBuilder::new()
        .with_class(title.into(), title.into())
        .with_title(title)
        .with_position(Position::Logical(LogicalPosition { x: 0., y: 0. }))
        .with_min_inner_size(Size::Logical(LogicalSize {
            width: 1920.,
            height: 1080.,
        }))
        .with_max_inner_size(Size::Logical(LogicalSize {
            width: 1920.,
            height: 1080.,
        }))
        .with_maximized(false)
        .build(&event_loop)
        .expect("Failed to open");

    let win_id = window.xlib_window().expect("Expected x window id");

    let bus = pipeline.bus().unwrap();

    bus.set_sync_handler(move |_, message| {
        if !is_video_overlay_prepare_window_handle_message(message) {
            return gst::BusSyncReply::Pass;
        }

        let source = message
            .src()
            .expect("Expected message to have a source")
            .dynamic_cast::<gstreamer_video::VideoOverlay>()
            .expect("Failed to cast source to video");

        unsafe {
            source.set_window_handle(win_id as usize);
        }

        gst::BusSyncReply::Pass
    });

    // Start playing
    pipeline
        .set_state(gst::State::Playing)
        .expect("Unable to set the pipeline to the `Playing` state");

    println!("Playing!");

    let _bus_loop = Some(std::thread::spawn(move || {
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            use gst::MessageView;

            match msg.view() {
                MessageView::Error(err) => {
                    eprintln!(
                        "Error received from element {:?}: {}",
                        err.src().map(|s| s.path_string()),
                        err.error()
                    );
                    eprintln!("Debugging information: {:?}", err.debug());
                    break;
                }
                MessageView::Eos(..) => break,
                _ => (),
            }
        }
    }));

    let mut pipeline_handle = Some(pipeline);

    event_loop.run(move |event, _, control_flow| {
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            pipeline_handle.take();

            *control_flow = ControlFlow::Exit;
        }
    });
}
