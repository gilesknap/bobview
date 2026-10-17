// The docs version switcher: a menu in the top bar listing the versions in
// switcher.json, which sits beside the version folders on gh-pages. A local
// `mdbook serve` has no switcher.json, so it shows no menu.
(function () {
    "use strict";
    const root = new URL(path_to_root || "./", window.location.href);
    const current = root.pathname.replace(/\/$/, "").split("/").pop();
    const page = window.location.pathname.slice(root.pathname.length);

    fetch(new URL("../switcher.json", root))
        .then((response) => (response.ok ? response.json() : []))
        .then((versions) => {
            if (!versions.length) return;
            const select = document.createElement("select");
            select.id = "version-switcher";
            select.title = "Docs version";
            for (const { version, url } of versions) {
                select.add(new Option(version, url, false, version === current));
            }
            select.addEventListener("change", async () => {
                // the same page in the other version, else its front page
                const target = new URL(page, select.value);
                const found = await fetch(target, { method: "HEAD" })
                    .then((r) => r.ok)
                    .catch(() => false);
                window.location.href = found ? target : select.value;
            });
            document.querySelector("#menu-bar .right-buttons")?.prepend(select);
        })
        .catch(() => {});
})();
