//! Native context-menu popup.
//!
//! The webview describes the menu as a flat tree of items (label + id, with
//! optional separators, accelerators, disabled state, and submenus); we
//! build a real `tauri::menu::Menu`, pop it under the cursor, and emit a
//! `context-menu-select` event with the chosen id back to the window. The
//! webview dispatches the action.
//!
//! We use Tauri's menu builder rather than touching `NSMenu` directly so
//! Linux/Windows still get a real OS menu (GTK / Win32) instead of the
//! Radix-rendered fallback. That fallback is still used in the browser
//! build where no Tauri runtime exists.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{
    menu::{
        ContextMenu, MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
    },
    AppHandle, Emitter, Manager, Runtime,
};

/// Item spec coming from the webview. `kind` discriminates the variant —
/// keeps the JSON shape obvious in DevTools and avoids `Option` churn.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ItemSpec {
    /// Regular clickable item.
    Item {
        id: String,
        label: String,
        #[serde(default)]
        accelerator: Option<String>,
        #[serde(default)]
        disabled: bool,
        /// If set, item renders with a leading checkmark (radio-group feel).
        #[serde(default)]
        checked: bool,
    },
    /// Visual divider; no id, no action.
    Separator,
    /// Nested submenu opened on hover.
    Submenu { label: String, items: Vec<ItemSpec> },
}

#[derive(Debug, Serialize, Clone)]
struct SelectEvent<'a> {
    id: &'a str,
}

/// Holds the most-recently-built popup menu so its `MenuItem` handles
/// outlive the on-click closures (Tauri requires the menu to stay alive
/// while it's onscreen). We swap on each popup.
pub struct ContextMenuState<R: Runtime> {
    last: Mutex<Option<tauri::menu::Menu<R>>>,
}

impl<R: Runtime> ContextMenuState<R> {
    pub fn new() -> Self {
        Self {
            last: Mutex::new(None),
        }
    }
}

fn build_item<R: Runtime>(app: &AppHandle<R>, spec: &ItemSpec) -> tauri::Result<BuiltItem<R>> {
    match spec {
        ItemSpec::Separator => Ok(BuiltItem::Separator(PredefinedMenuItem::separator(app)?)),
        ItemSpec::Item {
            id,
            label,
            accelerator,
            disabled,
            checked,
        } => {
            let display = if *checked {
                format!("✓ {label}")
            } else {
                label.clone()
            };
            let mut b = MenuItemBuilder::with_id(id, display);
            if let Some(a) = accelerator {
                b = b.accelerator(a);
            }
            if *disabled {
                b = b.enabled(false);
            }
            Ok(BuiltItem::Leaf(b.build(app)?))
        }
        ItemSpec::Submenu { label, items } => {
            let mut sub = SubmenuBuilder::new(app, label);
            for child in items {
                match build_item(app, child)? {
                    BuiltItem::Leaf(item) => sub = sub.item(&item),
                    BuiltItem::Separator(sep) => sub = sub.item(&sep),
                    BuiltItem::Submenu(inner) => sub = sub.item(&inner),
                }
            }
            Ok(BuiltItem::Submenu(sub.build()?))
        }
    }
}

enum BuiltItem<R: Runtime> {
    Leaf(MenuItem<R>),
    Separator(PredefinedMenuItem<R>),
    Submenu(tauri::menu::Submenu<R>),
}

/// Build the menu and pop it at the cursor. Returns immediately — the
/// selected id (or none) arrives later as a `context-menu-select` event
/// on the calling window. We can't easily make this `await` the choice
/// because the menu loop is driven by the OS, not Rust.
#[tauri::command]
pub fn show_context_menu<R: Runtime>(
    app: AppHandle<R>,
    items: Vec<ItemSpec>,
) -> Result<(), String> {
    let mut menu = MenuBuilder::new(&app);
    for spec in &items {
        let built = build_item(&app, spec).map_err(|e| e.to_string())?;
        menu = match built {
            BuiltItem::Leaf(item) => menu.item(&item),
            BuiltItem::Separator(sep) => menu.item(&sep),
            BuiltItem::Submenu(sub) => menu.item(&sub),
        };
    }
    let menu = menu.build().map_err(|e| e.to_string())?;

    // ContextMenu::popup expects the parent `tauri::Window`. WebviewWindow
    // wraps a Window but only exposes it indirectly: AsRef<Webview> hands
    // back the inner Webview, whose public `window()` returns the Window.
    let webview_window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    let webview: &tauri::Webview<R> = webview_window.as_ref();
    menu.popup(webview.window()).map_err(|e| e.to_string())?;

    if let Some(state) = app.try_state::<ContextMenuState<R>>() {
        if let Ok(mut slot) = state.last.lock() {
            *slot = Some(menu);
        }
    }
    Ok(())
}

/// Wired in `main.rs` via `on_menu_event` — when the user clicks an item in
/// any menu we built (popup or top-level), we emit `context-menu-select`
/// with its id. Top-level menu ids are namespaced; popup ids aren't, so
/// the webview can filter by listening to the right event.
pub fn handle_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit("context-menu-select", SelectEvent { id });
    }
}
