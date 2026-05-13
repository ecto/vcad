(function () {
  var status = document.getElementById("status");
  var continueLink = document.getElementById("continue");
  var progress = document.getElementById("progress");

  function hideProgress() {
    if (progress) progress.classList.add("hide");
  }

  try {
    if (window.opener && !window.opener.closed) {
      window.opener.postMessage(
        { type: "vcad:oauth-callback", url: window.location.href },
        window.location.origin,
      );
      status.textContent = "Signed in. You can close this window.";
      hideProgress();
      setTimeout(function () {
        try {
          window.close();
        } catch (_) {
          /* user closes it manually */
        }
      }, 50);
      return;
    }
  } catch (_) {
    // Cross-origin or detached opener — fall through to the standalone path.
  }

  status.textContent = "Sign-in complete.";
  hideProgress();
  continueLink.classList.remove("hide");
})();
