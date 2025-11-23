import { createApp } from 'vue'
import router from './router'
import App from './App.vue'

// 引入 UnoCSS
import 'virtual:uno.css'

const app = createApp(App)

app.use(router)
app.mount('#app')
