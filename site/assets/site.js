// Points the download buttons at the newest release's installers. Every
// GpuTerm release is a pre-release, which GitHub's /releases/latest skips,
// so the newest one is read from the API. Without it (offline, rate limit)
// the buttons keep their link to the release list.
(function () {
  var API = "https://api.github.com/repos/fortranmentis/GPUTERM/releases?per_page=10";
  var patterns = {
    windows: /_x64-setup\.exe$/,
    mac: /_aarch64\.dmg$/,
    deb: /_amd64\.deb$/,
    appimage: /_amd64\.AppImage$/,
  };

  fetch(API, { headers: { Accept: "application/vnd.github+json" } })
    .then(function (response) {
      return response.ok ? response.json() : [];
    })
    .then(function (releases) {
      var release = (releases || []).find(function (candidate) {
        return !candidate.draft && /^v\d/.test(candidate.tag_name);
      });
      if (!release) return;
      Object.keys(patterns).forEach(function (key) {
        var asset = release.assets.find(function (item) {
          return patterns[key].test(item.name);
        });
        document.querySelectorAll('[data-download="' + key + '"]').forEach(function (link) {
          if (asset) link.href = asset.browser_download_url;
        });
      });
      document.querySelectorAll("[data-release-version]").forEach(function (node) {
        node.textContent = release.tag_name.replace(/^v/, "");
      });
      document.querySelectorAll("[data-release-link]").forEach(function (link) {
        link.href = release.html_url;
      });
    })
    .catch(function () {});

  // Suggest the visitor's own OS first.
  var platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  var userAgent = navigator.userAgent || "";
  var mobile = /Android|iPhone|iPad|iPod/i.test(userAgent) || (/Mac/i.test(platform) && navigator.maxTouchPoints > 1);
  var os = mobile ? null : /Win/i.test(platform) ? "windows" : /Mac/i.test(platform) ? "mac" : /Linux/i.test(platform + userAgent) ? "deb" : null;
  if (os) {
    var preferred = document.querySelector('[data-download="' + os + '"]');
    if (preferred) {
      document.querySelectorAll(".downloads .button").forEach(function (button) {
        button.classList.remove("primary");
      });
      preferred.classList.add("primary");
      preferred.parentNode.insertBefore(preferred, preferred.parentNode.firstChild);
    }
  }
})();

// Fade sections in as they scroll into view.
(function () {
  if (!("IntersectionObserver" in window)) return;
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  var targets = document.querySelectorAll(".shot, section h2, .section-lead, .card, .phone-gallery figure, .terminal, .table-wrap");
  var observer = new IntersectionObserver(
    function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add("is-visible");
        observer.unobserve(entry.target);
      });
    },
    { rootMargin: "0px 0px -8% 0px" }
  );
  targets.forEach(function (node) {
    node.classList.add("reveal");
    observer.observe(node);
  });
})();
