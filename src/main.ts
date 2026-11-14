import { mount } from 'svelte';
import App from './App.svelte';
import EncodeWindow from './lib/features/convert/EncodeWindow.svelte';
import './app.css';

mount(
  new URLSearchParams(window.location.search).get('window') === 'quick-convert'
    ? EncodeWindow
    : App,
  { target: document.getElementById('app')! },
);
