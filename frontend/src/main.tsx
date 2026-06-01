import { render } from 'solid-js/web';
import './fonts.css';
import './styles.css';
import App from './App';

const root = document.getElementById('root');
if (!root) throw new Error('Missing app root');
render(() => <App />, root);
