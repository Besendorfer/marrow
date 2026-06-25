//! macOS application menu.
//!
//! We rebuild the standard menu so we can repurpose the close/quit accelerators
//! to the app's tab model. The native menu is the right place to do this: NSMenu
//! `performKeyEquivalent` runs ahead of the webview, so whatever a menu item binds
//! wins over the webview's default window handling.
//!
//! Bindings (macOS):
//!   - Cmd+T → `new_tab`   → emits `menu-new-tab` (open a new tab)
//!   - Cmd+W → `close_tab` → emits `menu-close-tab` (close the active tab, never the window)
//!   - Cmd+Q → `quit_request` → emits `menu-quit-request`. We don't quit directly:
//!             the frontend arms a "press ⌘Q again to quit" hint and only exits on a
//!             second press (Chrome-style), so an accidental Cmd+Q can't lose work.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

pub const NEW_TAB_ID: &str = "new_tab";
pub const CLOSE_TAB_ID: &str = "close_tab";
pub const QUIT_ID: &str = "quit_request";

pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    // First submenu becomes the bold application menu on macOS. Quit keeps the
    // standard Cmd+Q, but routes through the frontend's confirm-quit flow rather
    // than PredefinedMenuItem::quit (which would exit immediately).
    let quit = MenuItem::with_id(app, QUIT_ID, "Quit Marrow", true, Some("CmdOrCtrl+Q"))?;
    let app_menu = Submenu::with_items(
        app,
        "Marrow",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let new_tab = MenuItem::with_id(app, NEW_TAB_ID, "New Tab", true, Some("CmdOrCtrl+T"))?;
    let close_tab = MenuItem::with_id(app, CLOSE_TAB_ID, "Close Tab", true, Some("CmdOrCtrl+W"))?;
    // No Close Window item: the red traffic-light button still closes the window.
    // Cmd+W now belongs to Close Tab.
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &new_tab,
            &close_tab,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::minimize(app, None)?,
        ],
    )?;

    Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])
}
