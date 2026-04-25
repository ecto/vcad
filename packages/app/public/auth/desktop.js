// Desktop auth bridge.
//
// Supabase emails the user a magic-link that points here. We forward the
// click to vcad://auth/callback with the URL's query and fragment intact,
// so the Tauri deep-link plugin can hand the tokens to the desktop app.
//
// We deliberately do NOT load supabase-js on this page — `detectSessionInUrl`
// would burn the one-shot token_hash in the browser before the desktop ever
// got a chance to verify it.
(function () {
  var search = window.location.search || "";
  var hash = window.location.hash || "";
  var target = "vcad://auth/callback" + search + hash;

  var status = document.getElementById("status");
  var retryBtn = document.getElementById("retry");
  var webLink = document.getElementById("web");

  function go() {
    // location.replace keeps the magic-link URL out of the history stack
    // so the user can't accidentally re-click it after it has been used.
    window.location.replace(target);
  }

  function showFallback() {
    if (status) {
      status.textContent =
        "If vcad didn't open automatically, click below to try again. " +
        "If you don't have vcad installed, you can sign in on the web.";
    }
    if (retryBtn) {
      retryBtn.classList.remove("hide");
      retryBtn.addEventListener("click", go);
    }
    if (webLink) webLink.classList.remove("hide");
  }

  // Browsers don't fire a reliable event when a custom-scheme handler
  // succeeds or fails. Wait long enough that a registered handler would
  // have moved the user off this page; if we're still here, surface the
  // manual recovery affordances. The exact threshold is forgiving — most
  // OSes hand the URL off in well under a second.
  setTimeout(showFallback, 1500);

  go();
})();
