(function () {
  "use strict";

  // Keep regex in sync with bookmarklet in app/src/components/SettingsModal.tsx
  // and pr_parser.rs / utils.ts on the desktop side.
  function getPrRef() {
    var match = window.location.pathname.match(
      /^\/([A-Za-z0-9][A-Za-z0-9-]{0,38}\/[A-Za-z0-9._-]{1,100}\/pull\/\d+)/
    );
    return match ? match[1] : null;
  }

  function openDeepLink(url) {
    var frame = document.createElement("iframe");
    frame.style.display = "none";
    frame.src = url;
    document.body.appendChild(frame);
    setTimeout(function () { frame.remove(); }, 100);
  }

  function createButton(prRef) {
    var url = "relevantreviews://github.com/" + prRef;
    var btn = document.createElement("a");
    btn.href = url;
    btn.className = "rr-open-btn";
    btn.textContent = "Open in Relevant Reviews";
    btn.title = "Open this PR in the Relevant Reviews desktop app";
    btn.addEventListener("click", function (e) {
      e.preventDefault();
      openDeepLink(url);
      btn.textContent = "Opening...";
      setTimeout(function () {
        btn.textContent = "Open in Relevant Reviews";
      }, 2000);
    });
    return btn;
  }

  function inject() {
    if (document.querySelector(".rr-open-btn")) return false;
    var prRef = getPrRef();
    if (!prRef) return false;

    // New GitHub UI (Primer React): actions area in the page header
    var pageHeaderActions = document.querySelector('[data-component="PH_Actions"]');
    if (pageHeaderActions) {
      pageHeaderActions.appendChild(createButton(prRef));
      return true;
    }

    // Legacy GitHub UI
    var actionsBar = document.querySelector(".gh-header-actions");
    if (actionsBar) {
      actionsBar.prepend(createButton(prRef));
      return true;
    }

    var prTitle = document.querySelector(".js-issue-title");
    if (prTitle && prTitle.parentElement) {
      var btn = createButton(prRef);
      btn.style.marginLeft = "8px";
      prTitle.parentElement.appendChild(btn);
      return true;
    }

    return false;
  }

  if (inject()) {
    // Button placed on initial load; no observer needed yet
  }

  document.addEventListener("turbo:load", function () {
    var old = document.querySelector(".rr-open-btn");
    if (old) old.remove();
    inject();
    startObserver();
  });

  var debounceTimer = null;
  var observer = new MutationObserver(function () {
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(function () {
      if (document.querySelector(".rr-open-btn")) return;
      if (inject()) observer.disconnect();
    }, 150);
  });

  function startObserver() {
    observer.disconnect();
    observer.observe(document.body, { childList: true, subtree: true });
  }

  // Start observer if initial inject didn't find a target yet
  // (GitHub may still be rendering the PR header)
  if (!document.querySelector(".rr-open-btn")) {
    startObserver();
  }
})();
