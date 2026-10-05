(() => {
	try {
		const t = localStorage.getItem("yuuka-theme") || "dark";
		document.documentElement.setAttribute("data-theme", t);
		document.documentElement.classList.add("theme-no-transition");
		const colors = {
			dark: "#121212",
			light: "#FAFAFA",
			"blue-archive": "#FBFCFF",
		};
		const m = document.querySelector('meta[name="theme-color"]');
		if (m) m.setAttribute("content", colors[t] || colors.dark);
	} catch (_e) {}
})();
