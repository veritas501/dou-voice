import { useEffect, useRef } from "react";

import { fitMainWindow } from "../web/scripts/tauri-api.js";

/** 合并布局抖动。 */
const FIT_DEBOUNCE_MS = 48;
/** 小于该逻辑像素的变化忽略。 */
const FIT_SIZE_EPSILON = 2;

/**
 * 读取内容固有高度：临时去掉 min-height/height 填充，
 * 避免把“当前窗口高度”误当成“内容高度”造成无限增高。
 */
function measureIntrinsicHeight(target) {
  const prev = {
    height: target.style.height,
    minHeight: target.style.minHeight,
    maxHeight: target.style.maxHeight,
  };

  target.style.height = "auto";
  target.style.minHeight = "0px";
  target.style.maxHeight = "none";

  // 强制一次 reflow 后再量，避免分区切换后读到旧高度
  void target.offsetHeight;

  const height = Math.ceil(
    Math.max(target.scrollHeight, target.offsetHeight, target.clientHeight, 1),
  );

  target.style.height = prev.height;
  target.style.minHeight = prev.minHeight;
  target.style.maxHeight = prev.maxHeight;

  return height;
}

/**
 * 按内容固有高度适配主窗口（可增可减）。
 * 切换到更矮的分区（如 Auth）时会主动缩小窗口，去掉底部留白。
 */
export function useContentFitWindow(enabled = true, depsKey = "") {
  const rootRef = useRef(null);

  useEffect(() => {
    if (!enabled) return undefined;

    const root = rootRef.current;
    if (!root) return undefined;

    let disposed = false;
    let timerId = 0;
    let frameId = 0;
    let inFlight = false;
    let queued = false;
    /** 上次提交的内容高度；分区切换时 effect 重建会清零，确保可缩小 */
    let lastSentHeight = 0;
    /** 适配期间忽略 ResizeObserver，避免 set_size 回声 */
    let ignoreObserverUntil = 0;

    const runFit = async () => {
      if (disposed || inFlight) {
        queued = true;
        return;
      }

      const height = measureIntrinsicHeight(root);
      if (!Number.isFinite(height) || height <= 1) return;

      if (
        lastSentHeight > 0 &&
        Math.abs(height - lastSentHeight) < FIT_SIZE_EPSILON
      ) {
        return;
      }

      inFlight = true;
      ignoreObserverUntil = Date.now() + 240;
      try {
        await fitMainWindow({ contentHeight: height });
        if (disposed) return;
        // 以请求的内容高度为准，保证矮页可以缩小
        lastSentHeight = height;
      } catch (error) {
        console.warn("fit_main_window failed:", error);
      } finally {
        inFlight = false;
        if (queued && !disposed) {
          queued = false;
          scheduleFit();
        }
      }
    };

    const scheduleFit = () => {
      window.clearTimeout(timerId);
      timerId = window.setTimeout(() => {
        window.cancelAnimationFrame(frameId);
        frameId = window.requestAnimationFrame(() => {
          void runFit();
        });
      }, FIT_DEBOUNCE_MS);
    };

    const observer = new ResizeObserver(() => {
      if (Date.now() < ignoreObserverUntil) return;
      scheduleFit();
    });
    observer.observe(root);

    // 分区切换后等一帧再量，避开 framer-motion 入场未完成
    scheduleFit();
    const retryId = window.setTimeout(scheduleFit, 120);

    return () => {
      disposed = true;
      observer.disconnect();
      window.clearTimeout(timerId);
      window.clearTimeout(retryId);
      window.cancelAnimationFrame(frameId);
    };
  }, [enabled, depsKey]);

  return rootRef;
}
