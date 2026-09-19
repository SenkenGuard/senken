# Widget Host Protocol

A dashboard widget's document is a real, sandboxed iframe
(`sandbox="allow-scripts"`, opaque origin) that talks to the host over
`postMessage`. Every message shares this envelope:

```
widget → host: { channel: 'senken.widget', v: 1, id, method, params? }
host → widget: { channel: 'senken.widget', v: 1, id, ok, result?, error? }
```

`id` correlates a request with its reply.

## Methods a widget can call
| Method | Params | Result |
|---|---|---|
| `ready` | — | `{ widgetTypeId, config }` |
| `config.get` | — | `{ widgetTypeId, config }` |
| `config.patch` | `{ patch }` | `{ widgetTypeId, config }` (merged) |

## Messages the host sends unprompted
`theme.changed` — no `id`, never a reply to anything — sent when the widget
first says `ready`, and again every time the host's own theme changes:

```
{ channel: 'senken.widget', v: 1, method: 'theme.changed',
  params: { mode: 'dark' | 'light', tokens: { '--fg': '#f2f2ef', … } } }
```

`tokens` is a fixed set of CSS custom property names, always present:
`--bg --bg2 --chrome --card --card2 --pop --pop2 --fg --fg2 --inv --ink
--ink-shadow --dim --dim2 --gain --loss --font-sans --font-mono --radius`.

Apply with `document.documentElement.style.setProperty(name, value)`;
reference as `var(--fg, <fallback>)` in your CSS (the fallback renders
before the first `theme.changed`).

## A minimal widget, in full
```html
<script>
  let id = 0; const pending = new Map();
  function call(method, params) {
    const r = 'w' + (id += 1);
    return new Promise((resolve) => {
      pending.set(r, resolve);
      window.parent.postMessage({ channel: 'senken.widget', v: 1, id: r, method, params }, '*');
    });
  }
  window.addEventListener('message', (e) => {
    const d = e.data;
    if (!d || d.channel !== 'senken.widget' || d.v !== 1) return;
    if (d.method === 'theme.changed') {
      for (const [k, v] of Object.entries(d.params.tokens)) document.documentElement.style.setProperty(k, v);
      return;
    }
    pending.get(d.id)?.(d.result);
  });
  call('ready').then(({ config }) => {});
</script>
```
