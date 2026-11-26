import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './index.css';
import { createPlatformBridge } from './platform/bridge';

const bridge = createPlatformBridge();
const rootEl = document.getElementById('root');

if (rootEl) {
  createRoot(rootEl).render(
    <StrictMode>
      <App bridge={bridge} />
    </StrictMode>,
  );
}
