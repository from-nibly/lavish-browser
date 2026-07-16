document.querySelector('#script-status').value = 'relative script loaded';
document.querySelector('#clipboard').addEventListener('click', async () => {
  await navigator.clipboard.writeText('lavish-webkit-compatibility-fixture');
});
if (window.mermaid) window.mermaid.initialize({ startOnLoad: true });
