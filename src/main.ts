import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import './style.css';

const zone = document.querySelector<HTMLDivElement>('#drop-zone')!;
const fileTitle = document.querySelector<HTMLElement>('#file-title')!;
const fileSubtitle = document.querySelector<HTMLElement>('#file-subtitle')!;
const outputInput = document.querySelector<HTMLInputElement>('#output-path')!;
const status = document.querySelector<HTMLElement>('#status')!;
const progress = document.querySelector<HTMLElement>('#progress-bar')!;
const convertButton = document.querySelector<HTMLButtonElement>('#convert')!;
let inputPath = '';
let outputChosenByUser = false;

function setStatus(message: string, percent = 0) {
  status.textContent = message;
  progress.style.width = `${percent}%`;
  progress.classList.remove('error');
}

function setInput(path: string) {
  if (!path.toLowerCase().endsWith('.xd')) {
    setStatus('Нужен файл с расширением .xd');
    return;
  }
  inputPath = path;
  fileTitle.textContent = path.split(/[\\/]/).at(-1) || path;
  fileSubtitle.textContent = 'Файл выбран';
  if (!outputChosenByUser) {
    outputInput.value = path.replace(/\.xd$/i, '.figcraft');
  }
  convertButton.disabled = !outputInput.value;
  setStatus('Готово к конвертации');
}

async function chooseInput() {
  const selected = await open({ multiple: false, filters: [{ name: 'Adobe XD', extensions: ['xd'] }] });
  if (typeof selected === 'string') setInput(selected);
}

async function chooseOutput() {
  const selected = await save({ defaultPath: outputInput.value || 'Design.figcraft', filters: [{ name: 'FigCraft document', extensions: ['figcraft'] }] });
  if (selected) {
    outputInput.value = selected;
    outputChosenByUser = true;
    convertButton.disabled = !inputPath;
  }
}

document.querySelector('#browse-input')!.addEventListener('click', (event) => {
  event.stopPropagation();
  void chooseInput();
});
document.querySelector('#browse-output')!.addEventListener('click', () => void chooseOutput());
zone.addEventListener('click', () => void chooseInput());
zone.addEventListener('keydown', (event) => { if (event.key === 'Enter' || event.key === ' ') void chooseInput(); });
zone.addEventListener('dragover', (event) => { event.preventDefault(); zone.classList.add('dragging'); });
zone.addEventListener('dragleave', () => zone.classList.remove('dragging'));
zone.addEventListener('drop', (event) => {
  event.preventDefault(); zone.classList.remove('dragging');
  const file = event.dataTransfer?.files[0] as (File & { path?: string }) | undefined;
  if (file?.path) setInput(file.path);
  else setStatus('Не удалось получить путь файла. Нажмите «Выбрать файл».');
});

convertButton.addEventListener('click', async () => {
  if (!inputPath || !outputInput.value) return;
  convertButton.disabled = true;
  setStatus('Конвертация…');
  progress.classList.add('busy');
  try {
    await invoke('convert_xd_to_figcraft', { inputPath, outputPath: outputInput.value });
    setStatus(`Файл FigCraft сохранён: ${outputInput.value.split(/[\\/]/).at(-1)}`, 100);
  } catch (error) {
    setStatus(String(error));
    progress.classList.add('error');
  } finally {
    progress.classList.remove('busy');
    convertButton.disabled = false;
  }
});

// Tauri emits native file drops with filesystem paths, including on macOS.
if ('__TAURI_INTERNALS__' in window) {
  void getCurrentWindow().onDragDropEvent((event) => {
    if (event.payload.type === 'drop' && event.payload.paths[0]) setInput(event.payload.paths[0]);
  });
}
