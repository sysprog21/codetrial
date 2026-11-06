// Two helpers every part of the task page uses, in a module of their own so that
// code split out of task.js can import them without importing the page.

export const $ = (id) => document.getElementById(id);
export const id = () => crypto.randomUUID();
