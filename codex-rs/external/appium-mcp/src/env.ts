import { existsSync } from 'fs';
import log from './locators/logger.js';

const DEFAULT_ANDROID_HOME = '/Users/jqwang/Library/Android/sdk';

export function ensureAndroidEnv(): void {
  const androidHomeFromEnv = process.env.ANDROID_HOME;
  const defaultHomeExists = existsSync(DEFAULT_ANDROID_HOME);

  if (!androidHomeFromEnv && defaultHomeExists) {
    process.env.ANDROID_HOME = DEFAULT_ANDROID_HOME;
    log.info(`ANDROID_HOME 未设置，已默认使用 ${DEFAULT_ANDROID_HOME}`);
  }

  if (process.env.ANDROID_HOME) {
    const platformTools = `${process.env.ANDROID_HOME}/platform-tools`;
    const pathEntries = (process.env.PATH || '').split(':');
    if (!pathEntries.includes(platformTools)) {
      process.env.PATH = `${platformTools}:${process.env.PATH || ''}`;
      log.info('已将 platform-tools 目录加入 PATH');
    }
  } else {
    log.warn(
      'ANDROID_HOME 未设置，且默认路径 /Users/jqwang/Library/Android/sdk 不存在；请手动设置 ANDROID_HOME 并确保 platform-tools 在 PATH 中'
    );
  }
}
