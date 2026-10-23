//! Build-time constants. The coordinator may change `DEFAULT_PAGE_HOST`.

/// Host of the link page; links look like `https://<page_host>/#<ticket>`.
/// v0.5.2: Vercel, which also serves the short-link API (`/api/s`); GitHub Pages had no API.
pub const DEFAULT_PAGE_HOST: &str = "toss-link.vercel.app";

/// Name of the folder (under the platform data dir) holding settings, key, jobs.
pub const DATA_DIR_NAME: &str = "toss";

/// Env var that overrides the data dir (dev / tests: run two instances on one machine).
pub const DATA_DIR_ENV: &str = "TOSS_DATA_DIR";

/// CLI flag: start with the window hidden (autostart on a host machine).
pub const HIDDEN_FLAG: &str = "--hidden";

/// Label of the single main window.
pub const MAIN_WINDOW: &str = "main";
