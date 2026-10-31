import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { initPlatform } from './lib/api';
import { installWindowDrag } from './lib/windowDrag';

const view = new URLSearchParams(location.search).get('view') ?? 'main';

initPlatform().then((os) => {
  document.documentElement.dataset.os = os;
  // Gilgamesh's own window is opaque black (for a second monitor).
  if ((os === 'macos' || os === 'windows') && view !== 'gilgamesh') document.documentElement.classList.add('translucent');
  if (view === 'quick') document.documentElement.classList.add('quick');
  installWindowDrag();
  mount(App, { target: document.getElementById('app')!, props: { view } });
});
