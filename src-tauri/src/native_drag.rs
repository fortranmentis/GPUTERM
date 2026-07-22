use base64::Engine;
use std::{path::PathBuf, sync::mpsc::channel};
use tauri::{AppHandle, Window};

const PNG_DATA_URL_PREFIX: &str = "data:image/png;base64,";

#[tauri::command]
pub async fn start_native_file_drag(
    app: AppHandle,
    window: Window,
    paths: Vec<PathBuf>,
    image: String,
) -> Result<(), String> {
    validate_drag_paths(&paths)?;
    let image = decode_drag_image(&image)?;
    let (tx, rx) = channel();

    app.run_on_main_thread(move || {
        #[cfg(target_os = "linux")]
        let result = window
            .gtk_window()
            .map_err(|error| error.to_string())
            .and_then(|window| start_linux_file_drag(&window, paths, image));

        #[cfg(not(target_os = "linux"))]
        let result = drag::start_drag(
            &window,
            drag::DragItem::Files(paths),
            drag::Image::Raw(image),
            |_, _| {},
            drag::Options::default(),
        )
        .map_err(|error| error.to_string());

        let _ = tx.send(result);
    })
    .map_err(|error| error.to_string())?;

    rx.recv()
        .map_err(|_| "Native file drag initialization was interrupted".to_string())?
}

fn validate_drag_paths(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("No prepared local files are available to drag".to_string());
    }
    if let Some(path) = paths.iter().find(|path| !path.is_absolute()) {
        return Err(format!(
            "Native file drag requires an absolute path: {}",
            path.display()
        ));
    }
    Ok(())
}

fn decode_drag_image(image: &str) -> Result<Vec<u8>, String> {
    let encoded = image
        .strip_prefix(PNG_DATA_URL_PREFIX)
        .ok_or_else(|| "Native drag image must be a PNG data URL".to_string())?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("Invalid native drag image: {error}"))
}

#[cfg(target_os = "linux")]
fn start_linux_file_drag(
    window: &gtk::ApplicationWindow,
    paths: Vec<PathBuf>,
    image: Vec<u8>,
) -> Result<(), String> {
    use gdkx11::{
        gdk,
        glib::{self, ObjectExt, SignalHandlerId},
    };
    use gtk::{
        gdk_pixbuf,
        prelude::{DragContextExtManual, PixbufLoaderExt, WidgetExt, WidgetExtManual},
    };
    use std::{cell::RefCell, rc::Rc};

    let handler_ids = Rc::new(RefCell::new(Vec::<SignalHandlerId>::new()));
    window.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &[], gdk::DragAction::COPY);
    window.drag_source_add_uri_targets();

    let uris = paths
        .iter()
        .map(|path| {
            url::Url::from_file_path(path)
                .map(String::from)
                .map_err(|_| format!("Failed to create a file URI for {}", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    handler_ids
        .borrow_mut()
        .push(window.connect_drag_data_get(move |_, _, data, _, _| {
            let uri_refs = uris.iter().map(String::as_str).collect::<Vec<_>>();
            data.set_uris(&uri_refs);
        }));

    let target_list = window
        .drag_source_get_target_list()
        .ok_or_else(|| "Native drag target list is empty".to_string())?;
    let drag_context = window
        .drag_begin_with_coordinates(
            &target_list,
            gdk::DragAction::COPY,
            gdk::ffi::GDK_BUTTON1_MASK as i32,
            None,
            -1,
            -1,
        )
        .ok_or_else(|| "Failed to start the Linux file drag".to_string())?;

    let loader = gdk_pixbuf::PixbufLoader::new();
    if loader.write(&image).is_ok() && loader.close().is_ok() {
        if let Some(icon) = loader.pixbuf() {
            drag_context.drag_set_icon_pixbuf(&icon, 0, 0);
        }
    }

    // GTK/X11 requests text/uri-list asynchronously after drop-performed.
    // Keep drag-data-get connected until drag-end, otherwise Nautilus and
    // other GTK file managers receive an empty selection and create no file.
    let cleanup_window = window.clone();
    let cleanup_handlers = Rc::clone(&handler_ids);
    handler_ids
        .borrow_mut()
        .push(window.connect_drag_end(move |_, _| {
            let cleanup_window = cleanup_window.clone();
            let cleanup_handlers = Rc::clone(&cleanup_handlers);
            glib::idle_add_local_once(move || {
                for handler_id in cleanup_handlers.borrow_mut().drain(..) {
                    cleanup_window.disconnect(handler_id);
                }
                cleanup_window.drag_source_unset();
            });
        }));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_relative_drag_paths() {
        assert!(validate_drag_paths(&[]).is_err());
        assert!(validate_drag_paths(&[PathBuf::from("report.txt")]).is_err());
    }

    #[test]
    fn decodes_png_data_url() {
        assert_eq!(
            decode_drag_image("data:image/png;base64,iVBORw==").unwrap(),
            b"\x89PNG"
        );
        assert!(decode_drag_image("iVBORw==").is_err());
    }
}
