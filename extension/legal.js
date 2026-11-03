// Theme Toggle
    const themeBtn = document.getElementById("themeToggle");
    const themeLabel = document.getElementById("themeLabel");
    const htmlEl = document.documentElement;

    function applyTheme(theme) {
      htmlEl.setAttribute("data-theme", theme);
      localStorage.setItem("zoltraak_legal_theme", theme);
      themeLabel.textContent = theme === "dark" ? "Night shift" : "Day enamel";
    }

    const savedTheme = localStorage.getItem("zoltraak_legal_theme") ||
      (window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    applyTheme(savedTheme);

    themeBtn.addEventListener("click", () => {
      const current = htmlEl.getAttribute("data-theme");
      applyTheme(current === "dark" ? "light" : "dark");
    });

    // Copy Button for License
    document.querySelectorAll(".copy-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const code = btn.parentElement.querySelector("code").innerText;
        navigator.clipboard.writeText(code).then(() => {
          const original = btn.textContent;
          btn.textContent = "COPIED!";
          setTimeout(() => (btn.textContent = original), 1800);
        });
      });
    });

    // Search Filter
    const searchInput = document.getElementById("docSearch");
    searchInput.addEventListener("input", (e) => {
      const query = e.target.value.toLowerCase().trim();
      const chapters = document.querySelectorAll(".legal-chapter");

      chapters.forEach((chapter) => {
        if (!query) {
          chapter.style.display = "";
          return;
        }
        const text = chapter.textContent.toLowerCase();
        chapter.style.display = text.includes(query) ? "" : "none";
      });
    });

    // Keyboard shortcut for search
    window.addEventListener("keydown", (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        searchInput.focus();
      }
    });

    // ScrollSpy for Active Sidebar Link
    const sections = document.querySelectorAll(".legal-chapter");
    const navLinks = document.querySelectorAll(".nav-link");

    window.addEventListener("scroll", () => {
      let current = "";
      sections.forEach((section) => {
        const top = section.offsetTop - 80;
        if (window.pageYOffset >= top) {
          current = section.getAttribute("id");
        }
      });

      navLinks.forEach((link) => {
        link.classList.remove("active");
        if (link.getAttribute("href") === "#" + current) {
          link.classList.add("active");
        }
      });
    });
