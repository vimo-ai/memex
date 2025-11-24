import MarkdownIt from 'markdown-it'

const md = new MarkdownIt({
  html: false, // Disable HTML tags for security
  breaks: true, // Convert '\n' in paragraphs into <br>
  linkify: true, // Autoconvert URL-like text to links
  typographer: true, // Enable some language-neutral replacement + quotes beautification
})

// Custom renderer rules can be added here if needed
// For example, to add target="_blank" to links:
const defaultRender = md.renderer.rules.link_open || function (tokens, idx, options, _env, self) {
  return self.renderToken(tokens, idx, options);
};

md.renderer.rules.link_open = function (tokens, idx, options, env, self) {
  const token = tokens[idx];
  if (!token) return defaultRender(tokens, idx, options, env, self);

  // Ensure attrs exists
  if (!token.attrs) {
    token.attrs = [];
  }

  // Add target="_blank" to all links
  const aIndex = token.attrIndex('target');
  if (aIndex < 0) {
    token.attrPush(['target', '_blank']);
  } else {
    // We know attrs exists because we initialized it above
    if (token.attrs[aIndex]) {
      token.attrs[aIndex][1] = '_blank';
    }
  }

  // Add rel="noopener noreferrer" for security
  const relIndex = token.attrIndex('rel');
  if (relIndex < 0) {
    token.attrPush(['rel', 'noopener noreferrer']);
  } else {
    // We know attrs exists because we initialized it above
    if (token.attrs[relIndex]) {
      token.attrs[relIndex][1] = 'noopener noreferrer';
    }
  }

  return defaultRender(tokens, idx, options, env, self);
};

export function renderMarkdown(content: string): string {
  if (!content) return ''
  return md.render(content)
}
