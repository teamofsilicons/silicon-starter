// Native Solid controls using Silicon UI's source-owned foundation and component styles.
// https://ui.teamofsilicons.com — MIT license: /licenses/Silicon-UI.txt
import { splitProps, type JSX } from 'solid-js';
import { Dynamic } from 'solid-js/web';
import './silicon-ui/foundation.css';
import buttons from './silicon-ui/button.module.css';
import inputs from './silicon-ui/input.module.css';
import './silicon-ui/surfaces.css';

const classes = (base: string, name?: string, flags?: Record<string, boolean | undefined>) =>
  [base, name, ...Object.entries(flags || {}).filter(([, enabled]) => enabled).map(([key]) => key)].filter(Boolean).join(' ');

export function Button(props: JSX.ButtonHTMLAttributes<HTMLButtonElement>) {
  const [local, rest] = splitProps(props, ['class', 'classList']);
  const variant = () => local.class?.split(' ').includes('primary') ? buttons.primary : buttons.secondary;
  return <button {...rest} class={classes(`${buttons.button} ${variant()} silicon-button`, local.class, local.classList)} />;
}

export function Input(props: JSX.InputHTMLAttributes<HTMLInputElement>) {
  const [local, rest] = splitProps(props, ['class', 'classList']);
  return <input {...rest} class={classes(['checkbox', 'radio', 'range', 'color'].includes(props.type || '') ? '' : `${inputs.input} silicon-input`, local.class, local.classList)} />;
}

export function Card(props: JSX.HTMLAttributes<HTMLElement> & {as?: 'div' | 'article' | 'section'}) {
  const [local, rest] = splitProps(props, ['class', 'classList', 'as']);
  return <Dynamic component={local.as || 'section'} {...rest} class={classes('silicon-card', local.class, local.classList)} />;
}
