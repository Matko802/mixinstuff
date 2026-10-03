// ==UserScript==
// @name         Open in Mixtapes
// @namespace    https://github.com/m-obeid/Mixtapes
// @version      1.0.0
// @description  Hand the YouTube Music page you are on to Mixtapes: songs play, playlists, albums and artists open.
// @match        https://music.youtube.com/*
// @grant        GM_registerMenuCommand
// @grant        GM_getValue
// @grant        GM_setValue
// @run-at       document-idle
// @downloadURL  https://raw.githubusercontent.com/m-obeid/Mixtapes/main/extras/open-in-mixtapes.user.js
// @updateURL    https://raw.githubusercontent.com/m-obeid/Mixtapes/main/extras/open-in-mixtapes.user.js
// ==/UserScript==

(function () {
  "use strict";

  // Pages Mixtapes opens. Home, Explore and search stay in the browser.
  const OPENABLE = /^\/(watch|playlist|browse\/(MPREb|VL|UC)|channel\/|@)/;
  const AUTO_KEY = "autoOpen";

  const openable = () => OPENABLE.test(location.pathname);

  // The browser asks once whether to allow mixtapes:// links, then remembers.
  function openInMixtapes() {
    const url = new URL(location.href);
    // Playback position means nothing to Mixtapes, and the share tag is noise.
    url.searchParams.delete("t");
    url.searchParams.delete("si");
    location.href = "mixtapes://open?url=" + encodeURIComponent(url.toString());
  }

  function button() {
    const b = document.createElement("button");
    b.id = "open-in-mixtapes";
    b.textContent = "Open in Mixtapes";
    b.title = "Open this page in Mixtapes";
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
    b.addEventListener("click", openInMixtapes);
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
      openInMixtapes();
    }
  }
  setInterval(onNavigate, 500);
  onNavigate();

  GM_registerMenuCommand("Open in Mixtapes", openInMixtapes);
  GM_registerMenuCommand("Toggle opening links in Mixtapes automatically", () => {
    const next = !GM_getValue(AUTO_KEY, true);
    GM_setValue(AUTO_KEY, next);
    alert(next ? "Links to songs, playlists, albums and artists now open in Mixtapes." : "Links now stay in the browser. The button still hands a page over.");
  });
})();
