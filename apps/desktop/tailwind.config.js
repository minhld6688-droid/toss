/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: "media",
  theme: {
    extend: {
      colors: {
        bg: "rgb(var(--bg) / <alpha-value>)",
        card: "rgb(var(--card) / <alpha-value>)",
        line: "rgb(var(--line) / <alpha-value>)",
        ink: "rgb(var(--ink) / <alpha-value>)",
        mute: "rgb(var(--mute) / <alpha-value>)",
        accent: "rgb(var(--accent) / <alpha-value>)",
        "accent-ink": "rgb(var(--accent-ink) / <alpha-value>)",
        ok: "rgb(var(--ok) / <alpha-value>)",
        bad: "rgb(var(--bad) / <alpha-value>)",
        warn: "rgb(var(--warn) / <alpha-value>)",
      },
      fontFamily: {
        sans: ["'Bricolage Grotesque Variable'", "system-ui", "sans-serif"],
        display: ["'Fraunces Variable'", "Georgia", "serif"],
        mono: ["'JetBrains Mono Variable'", "ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
      },
      fontSize: { xs: ["12px", "16px"], sm: ["13px", "20px"], base: ["14px", "20px"], lg: ["16px", "24px"], xl: ["20px", "28px"], "2xl": ["24px", "30px"] },
      borderRadius: { DEFAULT: "12px", lg: "16px", xl: "20px", "2xl": "24px" },
      transitionTimingFunction: { aura: "cubic-bezier(0.32, 0.72, 0, 1)" },
    },
  },
  plugins: [],
};
