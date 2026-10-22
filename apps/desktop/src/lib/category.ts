// Treemap color = file category (by kind/extension). Color never encodes deletion safety;
// risk would always be stated in words (product-and-ux §7.1).

import type { Entry } from './types';
import { t } from '../i18n';

export type Category = 'folder' | 'media' | 'archive' | 'code' | 'document' | 'image' | 'other' | 'boundary';

const EXT: Record<string, Category> = {};
const put = (cat: Category, exts: string) => exts.split(' ').forEach((e) => (EXT[e] = cat));
put('media', 'mp4 mov mkv avi wmv webm m4v mp3 wav flac aac m4a ogg');
put('archive', 'zip 7z rar tar gz tgz bz2 xz zst iso dmg vhd vhdx img cab msi');
put('code', 'js ts tsx jsx rs go py java c h cpp cs json lock map wasm pdb obj o a lib dll exe so dylib node safetensors bin pt onnx gguf');
put('document', 'pdf doc docx xls xlsx ppt pptx txt md rtf odt csv log');
put('image', 'png jpg jpeg gif webp heic raw cr2 nef psd ai svg tif tiff bmp');

export function categoryOf(e: Pick<Entry, 'kind' | 'name' | 'boundary'>): Category {
  if (e.boundary) return 'boundary';
  if (e.kind === 'directory') return 'folder';
  const dot = e.name.lastIndexOf('.');
  if (dot <= 0) return 'other';
  return EXT[e.name.slice(dot + 1).toLowerCase()] ?? 'other';
}

export const CATEGORY_LABEL: Record<Category, string> = {
  get folder() { return t('category.categoryLabel.folder'); },
  get media() { return t('category.categoryLabel.media'); },
  get archive() { return t('category.categoryLabel.archive'); },
  get code() { return t('category.categoryLabel.code'); },
  get document() { return t('category.categoryLabel.document'); },
  get image() { return t('category.categoryLabel.image'); },
  get other() { return t('category.categoryLabel.other'); },
  get boundary() { return t('category.categoryLabel.boundary'); },
};
