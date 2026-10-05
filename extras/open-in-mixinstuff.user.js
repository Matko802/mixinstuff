// ==UserScript==
// @name         Open in Mixinstuff
// @namespace    https://github.com/Matko802/mixinstuff
// @version      1.0.0
// @description  Hand the YouTube Music page you are on to Mixinstuff: songs play, playlists, albums and artists open.
// @match        https://music.youtube.com/*
// @grant        GM_registerMenuCommand
// @grant        GM_getValue
// @grant        GM_setValue
// @run-at       document-idle
// @downloadURL  https://raw.githubusercontent.com/Matko802/mixinstuff/main/extras/open-in-mixinstuff.user.js
// @updateURL    https://raw.githubusercontent.com/Matko802/mixinstuff/main/extras/open-in-mixinstuff.user.js
// ==/UserScript==

(function () {
  "use strict";

  // Pages Mixinstuff opens. Home, Explore and search stay in the browser.
  const OPENABLE = /^\/(watch|playlist|browse\/(MPREb|VL|UC)|channel\/|@)/;
  const AUTO_KEY = "autoOpen";

  const openable = () => OPENABLE.test(location.pathname);

  // The browser asks once whether to allow mixinstuff:// links, then remembers.
  function openInMixinstuff() {
    const url = new URL(location.href);
    // Playback position means nothing to Mixinstuff, and the share tag is noise.
    url.searchParams.delete("t");
    url.searchParams.delete("si");
    location.href = "mixinstuff://open?url=" + encodeURIComponent(url.toString());
  }

  function button() {
    const b = document.createElement("button");
    b.id = "open-in-mixinstuff";
    b.textContent = "Open in Mixinstuff";
    b.title = "Open this page in Mixinstuff";
    Object.assign(b.style, {
      position: "fixed",
      right: "16px",
      bottom: "88px",
      zIndex: "9999",
      padding: "8px 14px",
      border: "none",
      borderRadius: "999px",
      background: "#8f4bc4",
      color: "#fff",
      font: "600 13px/1.2 Roboto, sans-serif",
      cursor: "pointer",
      boxShadow: "0 2px 8px rgba(0, 0, 0, 0.4)",
    });
    b.addEventListener("click", openInMixinstuff);
    document.body.appendChild(b);
    return b;
  }

  const b = button();
  let lastPath = null;

  // YouTube Music never reloads between pages, so the address is watched instead.
  function onNavigate() {
    b.style.display = openable() ? "" : "none";
    if (location.pathname + location.search === lastPath) {
      return;
    }
    const first = lastPath === null;
    lastPath = location.pathname + location.search;
    // Automatic handover only on a page opened from outside, not on every click inside the site.
    if (first && openable() && GM_getValue(AUTO_KEY, true)) {
      openInMixinstuff();
    }
  }
  setInterval(onNavigate, 500);
  onNavigate();

  GM_registerMenuCommand("Open in Mixinstuff", openInMixinstuff);
  GM_registerMenuCommand("Toggle opening links in Mixinstuff automatically", () => {
    const next = !GM_getValue(AUTO_KEY, true);
    GM_setValue(AUTO_KEY, next);
    alert(next ? "Links to songs, playlists, albums and artists now open in Mixinstuff." : "Links now stay in the browser. The button still hands a page over.");
  });
})();
