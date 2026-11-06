// Draw the book's Mermaid diagrams (the `mermaid` blocks mdbook-mermaid
// leaves as <pre class="mermaid">) in a light or dark theme to match the
// book's, and draw them again when the reader picks another theme.
(function () {
    "use strict";
    const darkThemes = ["ayu", "coal", "navy"];
    const html = document.documentElement;
    const diagrams = Array.from(document.querySelectorAll("pre.mermaid"));
    if (!diagrams.length || typeof mermaid === "undefined") return;
    for (const d of diagrams) d.dataset.source = d.textContent;

    const isDark = () => darkThemes.some((t) => html.classList.contains(t));
    let drawnDark = null;
    function draw() {
        const dark = isDark();
        if (dark === drawnDark) return;
        drawnDark = dark;
        mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
        for (const d of diagrams) {
            d.removeAttribute("data-processed");
            d.textContent = d.dataset.source;
        }
        mermaid.run({ nodes: diagrams });
    }

    draw();
    // mdBook switches theme by changing the class of <html>.
    new MutationObserver(draw).observe(html, { attributes: true, attributeFilter: ["class"] });
})();
