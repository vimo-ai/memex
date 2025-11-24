import { defineConfig, presetUno, presetIcons, presetWebFonts } from 'unocss'

export default defineConfig({
  presets: [
    presetUno(),
    presetIcons({
      scale: 1.2,
      cdn: 'https://esm.sh/',
    }),
    presetWebFonts({
      provider: 'google',
      fonts: {
        sans: 'Inter:400,500,600,700',
        mono: 'JetBrains Mono:400,500',
        display: 'Orbitron:400,500,700,900',
      },
    }),
  ],
  theme: {
    colors: {
      void: '#050505', // Deepest black
      neon: {
        cyan: '#00f3ff',
        violet: '#bd00ff',
        blue: '#0066ff',
      },
      surface: {
        100: '#111111',
        200: '#1a1a1a',
        300: '#222222',
      }
    },
  },
  shortcuts: {
    'bg-void': 'bg-void text-gray-300',
    'panel': 'bg-surface-100/50 backdrop-blur-md border border-white/5',
    'panel-hover': 'hover:bg-surface-200/50 hover:border-white/10 transition-all duration-300',

    // Text styles
    'text-glow': 'text-neon-cyan drop-shadow-[0_0_10px_rgba(0,243,255,0.5)]',
    'text-glow-violet': 'text-neon-violet drop-shadow-[0_0_10px_rgba(189,0,255,0.5)]',

    // Components
    'btn-neon': 'relative px-6 py-2 bg-transparent border border-neon-cyan/50 text-neon-cyan font-display uppercase tracking-wider hover:bg-neon-cyan/10 transition-all duration-300 hover:shadow-[0_0_20px_rgba(0,243,255,0.3)]',
    'input-void': 'bg-surface-100/50 border border-white/10 rounded-none px-4 py-3 text-white placeholder-gray-600 focus:outline-none focus:border-neon-cyan/50 focus:shadow-[0_0_15px_rgba(0,243,255,0.1)] transition-all font-mono',
  },
})
