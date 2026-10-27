(() => {
	try {
		// 廃止した blue-archive などの保存値はダークとして扱う
		const t =
			localStorage.getItem("yuuka-theme") === "light" ? "light" : "dark";
		document.documentElement.setAttribute("data-theme", t);
		document.documentElement.classList.add("theme-no-transition");
		const colors = {
			dark: "#121212",
			light: "#FAFAFA",
		};
		const m = document.querySelector('meta[name="theme-color"]');
		if (m) m.setAttribute("content", colors[t] || colors.dark);
	} catch (_e) {}
})();
