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
  var os = /Win/i.test(platform) ? "windows" : /Mac/i.test(platform) ? "mac" : /Linux/i.test(platform + userAgent) ? "deb" : null;
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
