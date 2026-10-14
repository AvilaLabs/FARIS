// SPDX-License-Identifier: AGPL-3.0-only
// Adds the documentation licence statement to the foot of every handbook page (DOC-037).
(function () {
  "use strict";
  var main = document.querySelector("#mdbook-content main, #content main, main");
  if (!main || main.querySelector(".licence-footer")) {
    return;
  }
  var note = document.createElement("p");
  note.className = "licence-footer";
  note.style.cssText = "margin-top:2em;padding-top:1em;border-top:1px solid currentColor;opacity:0.7;font-size:0.85em";
  note.appendChild(document.createTextNode(
    "This handbook is licensed under the GNU Affero General Public License, version 3 only (AGPL-3.0-only), " +
    "the same licence as the FARIS source code. Third-party text, data and figures keep their own licences and " +
    "are credited where they appear. "));
  var link = document.createElement("a");
  link.href = "https://github.com/AvilaLabs/FARIS/blob/main/LICENSE";
  link.textContent = "Licence text";
  note.appendChild(link);
  note.appendChild(document.createTextNode("."));
  main.appendChild(note);
})();
