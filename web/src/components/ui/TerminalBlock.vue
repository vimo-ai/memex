<script setup lang="ts">
import { computed, ref } from 'vue'
import { renderMarkdown } from '@/utils/markdown'

const props = defineProps<{
  content: string
  type: 'user' | 'assistant' | 'tool' | 'system'
  timestamp: string
  toolName?: string
  toolArgs?: string
}>()

// 工具消息展开状态
const isExpanded = ref(false)

function formatTime(timestamp: string): string {
  return new Date(timestamp).toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

// 工具图标映射
const toolIcons: Record<string, string> = {
  Read: 'i-carbon-document',
  Write: 'i-carbon-document-add',
  Edit: 'i-carbon-edit',
  Bash: 'i-carbon-terminal',
  Glob: 'i-carbon-search',
  Grep: 'i-carbon-search-locate',
  Task: 'i-carbon-task',
  TaskCreate: 'i-carbon-task-add',
  TaskUpdate: 'i-carbon-task-complete',
  TaskList: 'i-carbon-list',
  WebFetch: 'i-carbon-cloud-download',
  WebSearch: 'i-carbon-search',
  AskUserQuestion: 'i-carbon-help',
  TodoWrite: 'i-carbon-checkbox-checked',
}

// 获取工具图标
function getToolIcon(name: string): string {
  return toolIcons[name] || 'i-carbon-tool-box'
}

// 工具分类
type ToolCategory = 'file' | 'search' | 'task' | 'shell' | 'web' | 'other'

function getToolCategory(name: string): ToolCategory {
  if (['Read', 'Write', 'Edit', 'NotebookEdit'].includes(name)) return 'file'
  if (['Glob', 'Grep'].includes(name)) return 'search'
  if (['Task', 'TaskCreate', 'TaskUpdate', 'TaskList', 'TaskGet', 'TodoWrite'].includes(name)) return 'task'
  if (['Bash'].includes(name)) return 'shell'
  if (['WebFetch', 'WebSearch'].includes(name)) return 'web'
  return 'other'
}

// 工具分类颜色
const categoryColors: Record<ToolCategory, { border: string; bg: string; text: string }> = {
  file: { border: 'border-blue-500/50', bg: 'bg-blue-500/5', text: 'text-blue-400' },
  search: { border: 'border-yellow-500/50', bg: 'bg-yellow-500/5', text: 'text-yellow-400' },
  task: { border: 'border-purple-500/50', bg: 'bg-purple-500/5', text: 'text-purple-400' },
  shell: { border: 'border-green-500/50', bg: 'bg-green-500/5', text: 'text-green-400' },
  web: { border: 'border-cyan-500/50', bg: 'bg-cyan-500/5', text: 'text-cyan-400' },
  other: { border: 'border-orange-400/50', bg: 'bg-orange-400/5', text: 'text-orange-400' },
}

// 解析工具参数为结构化数据
interface ParsedArgs {
  filePath?: string
  command?: string
  pattern?: string
  content?: string
  description?: string
  taskId?: string
  status?: string
  subject?: string
  query?: string
  url?: string
  oldString?: string
  newString?: string
  raw?: string
}

function parseToolArgs(args: string | undefined): ParsedArgs {
  if (!args) return {}
  try {
    const parsed = JSON.parse(args)
    return {
      filePath: parsed.file_path || parsed.filePath || parsed.path || parsed.notebook_path,
      command: parsed.command,
      pattern: parsed.pattern,
      content: parsed.content,
      description: parsed.description,
      taskId: parsed.taskId || parsed.task_id,
      status: parsed.status,
      subject: parsed.subject,
      query: parsed.query || parsed.q,
      url: parsed.url,
      oldString: parsed.old_string || parsed.oldString,
      newString: parsed.new_string || parsed.newString,
      raw: args,
    }
  } catch {
    return { raw: args }
  }
}

// 缩短文件路径显示
function shortenPath(path: string): string {
  const parts = path.split('/')
  if (parts.length <= 4) return path
  return `.../${parts.slice(-3).join('/')}`
}

// 解析的工具内容类型
interface ParsedToolContent {
  toolName: string
  category: ToolCategory
  args: ParsedArgs
}

// 解析的工具内容 - 有 toolName 就显示工具UI
const parsedToolContent = computed<ParsedToolContent | null>(() => {
  // 有 toolName 字段就是工具调用
  if (props.toolName) {
    return {
      toolName: props.toolName,
      category: getToolCategory(props.toolName),
      args: parseToolArgs(props.toolArgs),
    }
  }

  // 尝试从 content 解析 [Tool: xxx] 格式
  if (props.type === 'tool' && props.content) {
    const match = props.content.match(/^\[Tool:\s*([^\]]+)\]\s*(.*)$/)
    if (match && match[1]) {
      const toolName = match[1]
      return {
        toolName,
        category: getToolCategory(toolName),
        args: parseToolArgs(match[2] ?? ''),
      }
    }
  }

  return null
})

// 是否为工具消息
const isToolMessage = computed(() => parsedToolContent.value !== null)

// 工具颜色
const toolColors = computed(() => {
  if (!parsedToolContent.value) return categoryColors.other
  return categoryColors[parsedToolContent.value.category]
})

// 渲染普通内容
const renderedContent = computed(() => {
  if (isToolMessage.value) return ''
  return renderMarkdown(props.content)
})

// 是否为空消息
const isEmpty = computed(() => !props.content && !props.toolName)
</script>

<template>
  <div v-if="!isEmpty" class="font-mono text-sm leading-relaxed mb-6 group max-w-[85%]">
    <!-- Header -->
    <div
      class="flex items-center gap-2 mb-1 opacity-50 group-hover:opacity-100 transition-opacity select-none"
      :class="{ 'flex-row-reverse': type === 'user' }"
    >
      <span :class="type === 'user' ? 'text-neon-cyan' : type === 'assistant' ? 'text-neon-violet' : type === 'system' ? 'text-gray-400' : 'text-orange-400'">
        {{ type === 'user' ? 'root@user' : type === 'assistant' ? 'memex@ai' : type === 'system' ? 'sys@info' : 'tool@call' }}
      </span>
      <span class="text-gray-600">::</span>
      <span class="text-gray-500">[{{ formatTime(timestamp) }}]</span>
      <span class="text-gray-600">{{ type === 'user' ? '<<' : '>>' }}</span>
    </div>

    <!-- Tool Message UI -->
    <div v-if="isToolMessage && parsedToolContent" class="border-l-2 pl-4 py-2" :class="[toolColors.border, toolColors.bg]">
      <!-- Tool Header -->
      <div class="flex items-center gap-2 mb-2">
        <div :class="[getToolIcon(parsedToolContent.toolName), toolColors.text]" />
        <code :class="['font-bold text-sm', toolColors.text]">{{ parsedToolContent.toolName }}</code>
        <button
          v-if="parsedToolContent.args.raw"
          class="text-[10px] text-gray-400 hover:text-white transition-colors px-1.5 py-0.5 rounded border border-gray-600 hover:border-gray-400 ml-auto"
          @click="isExpanded = !isExpanded"
        >
          {{ isExpanded ? 'HIDE' : 'RAW' }}
        </button>
      </div>

      <!-- File Tools: Read/Write/Edit -->
      <template v-if="parsedToolContent.category === 'file'">
        <div v-if="parsedToolContent.args.filePath" class="flex items-center gap-2 text-xs">
          <span class="text-gray-500">PATH:</span>
          <code class="text-gray-300 bg-black/30 px-2 py-0.5 rounded">{{ shortenPath(parsedToolContent.args.filePath) }}</code>
        </div>
        <div v-if="parsedToolContent.toolName === 'Edit' && parsedToolContent.args.oldString" class="mt-2 text-xs">
          <div class="text-gray-500 mb-1">REPLACE:</div>
          <div class="flex gap-2 items-start">
            <code class="text-red-400/70 bg-red-500/10 px-2 py-1 rounded text-[11px] max-w-[45%] overflow-hidden whitespace-nowrap text-ellipsis">{{ parsedToolContent.args.oldString.slice(0, 50) }}{{ parsedToolContent.args.oldString.length > 50 ? '...' : '' }}</code>
            <span class="text-gray-600">→</span>
            <code class="text-green-400/70 bg-green-500/10 px-2 py-1 rounded text-[11px] max-w-[45%] overflow-hidden whitespace-nowrap text-ellipsis">{{ parsedToolContent.args.newString?.slice(0, 50) }}{{ (parsedToolContent.args.newString?.length || 0) > 50 ? '...' : '' }}</code>
          </div>
        </div>
      </template>

      <!-- Search Tools: Glob/Grep -->
      <template v-else-if="parsedToolContent.category === 'search'">
        <div v-if="parsedToolContent.args.pattern" class="flex items-center gap-2 text-xs">
          <span class="text-gray-500">PATTERN:</span>
          <code class="text-yellow-300 bg-black/30 px-2 py-0.5 rounded font-mono">{{ parsedToolContent.args.pattern }}</code>
        </div>
        <div v-if="parsedToolContent.args.filePath" class="flex items-center gap-2 text-xs mt-1">
          <span class="text-gray-500">SCOPE:</span>
          <code class="text-gray-400 bg-black/30 px-2 py-0.5 rounded">{{ shortenPath(parsedToolContent.args.filePath) }}</code>
        </div>
      </template>

      <!-- Shell Tools: Bash -->
      <template v-else-if="parsedToolContent.category === 'shell'">
        <div v-if="parsedToolContent.args.command" class="text-xs">
          <code class="text-green-300 bg-black/40 px-2 py-1 rounded block font-mono whitespace-pre-wrap break-all">$ {{ parsedToolContent.args.command }}</code>
        </div>
        <div v-if="parsedToolContent.args.description" class="text-[11px] text-gray-500 mt-1">
          {{ parsedToolContent.args.description }}
        </div>
      </template>

      <!-- Task Tools -->
      <template v-else-if="parsedToolContent.category === 'task'">
        <div v-if="parsedToolContent.args.subject" class="flex items-center gap-2 text-xs">
          <span class="text-gray-500">TASK:</span>
          <span class="text-purple-300">{{ parsedToolContent.args.subject }}</span>
        </div>
        <div v-if="parsedToolContent.args.taskId" class="flex items-center gap-2 text-xs">
          <span class="text-gray-500">ID:</span>
          <code class="text-purple-400 bg-black/30 px-1.5 py-0.5 rounded">#{{ parsedToolContent.args.taskId }}</code>
          <span v-if="parsedToolContent.args.status" class="px-1.5 py-0.5 rounded text-[10px]" :class="{
            'bg-green-500/20 text-green-400': parsedToolContent.args.status === 'completed',
            'bg-blue-500/20 text-blue-400': parsedToolContent.args.status === 'in_progress',
            'bg-gray-500/20 text-gray-400': parsedToolContent.args.status === 'pending',
          }">{{ parsedToolContent.args.status }}</span>
        </div>
        <div v-if="parsedToolContent.args.description" class="text-[11px] text-gray-400 mt-1 line-clamp-2">
          {{ parsedToolContent.args.description }}
        </div>
      </template>

      <!-- Web Tools -->
      <template v-else-if="parsedToolContent.category === 'web'">
        <div v-if="parsedToolContent.args.url" class="flex items-center gap-2 text-xs">
          <span class="text-gray-500">URL:</span>
          <code class="text-cyan-300 bg-black/30 px-2 py-0.5 rounded truncate max-w-[80%]">{{ parsedToolContent.args.url }}</code>
        </div>
        <div v-if="parsedToolContent.args.query" class="flex items-center gap-2 text-xs mt-1">
          <span class="text-gray-500">QUERY:</span>
          <span class="text-cyan-200">{{ parsedToolContent.args.query }}</span>
        </div>
      </template>

      <!-- Other Tools: Show key info -->
      <template v-else>
        <div v-if="parsedToolContent.args.description" class="text-xs text-gray-400">
          {{ parsedToolContent.args.description }}
        </div>
        <div v-else-if="!isExpanded && parsedToolContent.args.raw" class="text-xs text-gray-500 truncate">
          {{ parsedToolContent.args.raw.slice(0, 100) }}{{ parsedToolContent.args.raw.length > 100 ? '...' : '' }}
        </div>
      </template>

      <!-- Raw JSON (expanded) -->
      <pre v-if="isExpanded && parsedToolContent.args.raw" class="text-[11px] text-gray-500 mt-2 p-2 bg-black/40 rounded overflow-x-auto max-h-48 overflow-y-auto">{{ JSON.stringify(JSON.parse(parsedToolContent.args.raw), null, 2) }}</pre>
    </div>

    <!-- Regular Content -->
    <div
      v-else
      class="markdown-content break-words py-2"
      :class="[
        type === 'user'
          ? 'text-white border-r-2 border-neon-cyan/50 bg-neon-cyan/5 pr-4 pl-2 text-right'
          : type === 'system'
          ? 'text-gray-400 border-l-2 border-gray-500/50 bg-gray-500/5 pl-4 italic'
          : 'text-gray-300 border-l-2 border-neon-violet/50 pl-4'
      ]"
    >
      <div class="text-left" v-html="renderedContent" />
    </div>
  </div>
</template>
