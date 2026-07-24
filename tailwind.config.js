/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        background: 'var(--color-bg)',
        'bg-secondary': 'var(--color-bg-secondary)',
        surface: 'var(--color-surface)',
        'surface-hover': 'var(--color-surface-hover)',
        'surface-active': 'var(--color-surface-active)',
        border: 'var(--color-border)',
        'border-subtle': 'var(--color-border-subtle)',
        // Liquid Glass chrome materials; composites live in index.css as
        // .app-glass-* (they carry blur, highlight and shadow too).
        glass: {
          DEFAULT: 'var(--glass)',
          soft: 'var(--glass-soft)',
          strong: 'var(--glass-strong)',
          border: 'var(--glass-border)',
          hairline: 'var(--glass-hairline)',
        },
        accent: {
          DEFAULT: 'var(--color-accent)',
          light: 'var(--color-accent-light)',
          dark: 'var(--color-accent-dark)',
          bg: 'var(--color-accent-bg)',
          border: 'var(--color-accent-border)',
        },
        danger: {
          DEFAULT: 'var(--color-danger)',
          bg: 'var(--color-danger-bg)',
        },
      },
      textColor: {
        primary: 'var(--color-text-primary)',
        secondary: 'var(--color-text-secondary)',
        tertiary: 'var(--color-text-tertiary)',
        muted: 'var(--color-text-muted)',
        faint: 'var(--color-text-faint)',
      },
      fontFamily: {
        sans: [
          '"SF Pro Text"',
          '"PingFang SC"',
          '"Hiragino Sans GB"',
          '"Noto Sans SC"',
          '"Microsoft YaHei"',
          '-apple-system',
          'BlinkMacSystemFont',
          '"Segoe UI"',
          'system-ui',
          'sans-serif',
        ],
        mono: [
          '"SF Mono"',
          '"Fira Code"',
          '"JetBrains Mono"',
          'Menlo',
          'Monaco',
          'monospace',
        ],
      },
    },
  },
  plugins: [],
}
