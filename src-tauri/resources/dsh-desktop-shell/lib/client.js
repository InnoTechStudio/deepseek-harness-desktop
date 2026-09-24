window.__ModuleLoader__.load({
	id: "dsh-desktop-shell",
	factory: (require) => {
		var module = { exports: {} };
		var exports = module.exports;
		Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
		//#region \0rolldown/runtime.js
		var __create = Object.create;
		var __defProp = Object.defineProperty;
		var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
		var __getOwnPropNames = Object.getOwnPropertyNames;
		var __getProtoOf = Object.getPrototypeOf;
		var __hasOwnProp = Object.prototype.hasOwnProperty;
		var __copyProps = (to, from, except, desc) => {
			if (from && typeof from === "object" || typeof from === "function") for (var keys = __getOwnPropNames(from), i = 0, n = keys.length, key; i < n; i++) {
				key = keys[i];
				if (!__hasOwnProp.call(to, key) && key !== except) __defProp(to, key, {
					get: ((k) => from[k]).bind(null, key),
					enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable
				});
			}
			return to;
		};
		var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(isNodeMode || !mod || !mod.__esModule || !__hasOwnProp.call(mod, "default") ? __defProp(target, "default", {
			value: mod,
			enumerable: true
		}) : target, mod));
		//#endregion
		let _deepseek_ai_dsh_client_ui_primitives = require("@deepseek-ai/dsh-client-ui-primitives");
		_deepseek_ai_dsh_client_ui_primitives = __toESM(_deepseek_ai_dsh_client_ui_primitives, 1);
		let react = require("react");
		let react_jsx_runtime = require("react/jsx-runtime");
		const SIDEBAR_AUTO_COLLAPSE = 1024;
		/**
		* Resolve three desktop columns without allowing details to squeeze the conversation below its floor.
		* @param viewport - available frame width.
		* @param sidebar - sidebar preference, where zero selects the compact rail.
		* @param details - details preference, where zero closes the panel.
		* @returns rendered column widths.
		*/
		function computeDesktopColumns(viewport, sidebar, details, collapsedWidth = 56) {
			const sidebarWidth = sidebar === 0 ? collapsedWidth : clamp(sidebar, 264, 420);
			const preferredDetails = details === 0 ? 0 : clamp(details, 300, 520);
			if (sidebarWidth + preferredDetails + 400 <= viewport) return {
				sidebar: sidebarWidth,
				center: viewport - sidebarWidth - preferredDetails,
				details: preferredDetails
			};
			const reducedDetails = preferredDetails === 0 ? 0 : Math.max(300, viewport - sidebarWidth - 400);
			if (sidebarWidth + reducedDetails + 400 <= viewport) return {
				sidebar: sidebarWidth,
				center: 400,
				details: reducedDetails
			};
			return {
				sidebar: sidebarWidth,
				center: Math.max(0, viewport - sidebarWidth),
				details: 0
			};
		}
		function clamp(value, min, max) {
			return Math.min(max, Math.max(min, Math.round(value)));
		}
		/**
		* Small observable panel controller used by the advanced root registration.
		*
		* Also implements the official `ILayout` methods (`beginNavigation`, `selectPanel`,
		* `openRightbar`, `closeRightbar`). The desktop shell disables `ui-layout`, so
		* nothing else provides `LayoutController`. ui-workspace's 新建会话 calls
		* `beginNavigation()` then `selectPanel(null)`; missing either is a TypeError
		* swallowed as `console.warn("new session failed:")`, and the app looks idle.
		*/
		const INITIAL_PANEL_INFO = Object.freeze({ activePanelId: null });
		var DesktopLayoutState = class {
			snapshot = Object.freeze({
				sidebar: 280,
				details: 0,
				narrow: false,
				narrowExpanded: false,
				activePanelId: null,
				panelInfo: INITIAL_PANEL_INFO
			});
			listeners = /* @__PURE__ */ new Set();
			navigation = new AbortController();
			/** @returns the immutable current panel snapshot. */
			getSnapshot() {
				return this.snapshot;
			}
			/** @param listener - callback notified after a snapshot replacement. @returns its disposer. */
			subscribe(listener) {
				this.listeners.add(listener);
				return () => {
					this.listeners.delete(listener);
				};
			}
			/** Toggle the wide sidebar and the platform-selected compact rail. */
			toggleSidebar() {
				if (this.snapshot.narrow) {
					this.publish({
						...this.snapshot,
						narrowExpanded: !this.snapshot.narrowExpanded
					});
					return;
				}
				this.publish({
					...this.snapshot,
					sidebar: this.snapshot.sidebar === 0 ? 280 : 0
				});
			}
			/** @param narrow - whether the frame is below the automatic-collapse breakpoint. */
			setNarrow(narrow) {
				if (this.snapshot.narrow === narrow) return;
				this.publish({
					...this.snapshot,
					narrow,
					narrowExpanded: false
				});
			}
			/** Open details at its default width. */
			openDetails() {
				if (this.snapshot.details === 0) this.publish({
					...this.snapshot,
					details: 360
				});
			}
			/** Close details while keeping its slot mounted. */
			closeDetails() {
				if (this.snapshot.details !== 0) this.publish({
					...this.snapshot,
					details: 0
				});
			}
			/** @param width - requested sidebar width from a resize gesture. */
			setSidebar(width) {
				this.publish({
					...this.snapshot,
					sidebar: clamp(width, 264, 420)
				});
			}
			/** @param width - requested details width from a resize gesture. */
			setDetails(width) {
				this.publish({
					...this.snapshot,
					details: clamp(width, 300, 520)
				});
			}
			/**
			* Start an asynchronous navigation, aborting any earlier pending one.
			* ui-workspace wraps this with `AbortSignal.any` and checks it before
			* committing `sessions.open`.
			*/
			beginNavigation() {
				this.navigation.abort();
				this.navigation = new AbortController();
				return this.navigation.signal;
			}
			/**
			* Official main-slot selection. `null` shows the conversation; a non-null id
			* is stored so `renderSlot("main", {}, { entryKey })` can switch panels.
			*
			* Official `LayoutController` throws if the id is not yet registered. Desktop
			* does not: sidebar PanelRow may select a panel before its `main` occupant
			* has injected. Unknown ids still become `activePanelId` so the keyed slot
			* can resolve them once they appear.
			*/
			selectPanel(panelId) {
				this.navigation.abort();
				const next = panelId === null ? null : panelId;
				if (this.snapshot.activePanelId === next) return;
				this.publish({
					...this.snapshot,
					activePanelId: next
				});
			}
			/**
			* Reset `activePanelId` when the selected main key is no longer registered.
			* Mirrors official `retainMainPanels`.
			*/
			retainMainPanels(panelIds) {
				const current = this.snapshot.activePanelId;
				if (current === null || panelIds.includes(current)) return;
				this.publish({
					...this.snapshot,
					activePanelId: null
				});
			}
			/** Snapshot consumed by the official `usePanelInfo` root hook. */
			getPanelInfo() {
				return this.snapshot.panelInfo;
			}
			/** Map the official rightbar show onto the desktop details column. */
			openRightbar(_track, _fullscreen) {
				this.openDetails();
			}
			/** Map the official rightbar hide onto the desktop details column. */
			closeRightbar() {
				this.closeDetails();
			}
			/** Abort pending navigations when the layout owner is unloaded. */
			dispose() {
				this.navigation.abort();
			}
			publish(next) {
				const panelInfo = next.activePanelId === this.snapshot.panelInfo.activePanelId ? this.snapshot.panelInfo : Object.freeze({ activePanelId: next.activePanelId });
				this.snapshot = Object.freeze({
					...next,
					panelInfo
				});
				for (const listener of this.listeners) listener();
			}
		};
		//#endregion
		//#region src/client/AdvancedFrame.tsx
		function postDesktopMessage(type) {
			window.parent.postMessage({ type }, "*");
		}
		/**
		* 宿主是否报告有可用更新。
		*
		* 标题栏只在有更新时才显示按钮，所以这个状态必须来自宿主的真实检查结果，
		* 不能由前端自己猜。挂载时主动问一次：后台检查任务启动 20 秒后才跑第一轮，
		* 而这个组件通常更早就绪。
		*/
		function useUpdateAvailable() {
			const [available, setAvailable] = (0, react.useState)(false);
			(0, react.useEffect)(() => {
				const onMessage = (event) => {
					if (event.source !== window.parent) return;
					const message = event.data;
					if (message === null || typeof message !== "object") return;
					if (message.type !== "dsh-desktop:update-availability") return;
					setAvailable(message.available === true);
				};
				window.addEventListener("message", onMessage);
				postDesktopMessage("dsh-desktop:request-update-state");
				return () => {
					window.removeEventListener("message", onMessage);
				};
			}, []);
			return available;
		}
		/**
		* 向上的箭头，从一条横线上抬起——比裸箭头更像"升级"而不是"滚动到顶部"。
		*
		* 底线在下、箭头在上，整体重心与 12px 的文字基线对齐，
		* 两端按钮共用同一尺寸，视觉上才一致。
		*/
		function UpdateArrowIcon() {
			return /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("svg", {
				width: "14",
				height: "14",
				viewBox: "0 0 16 16",
				fill: "none",
				"aria-hidden": "true",
				children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)("path", {
					d: "M8 10V3.6M8 3.6L5.2 6.4M8 3.6L10.8 6.4",
					stroke: "currentColor",
					strokeWidth: "1.7",
					strokeLinecap: "round",
					strokeLinejoin: "round"
				}), /* @__PURE__ */ (0, react_jsx_runtime.jsx)("path", {
					d: "M4.4 12.6h7.2",
					stroke: "currentColor",
					strokeWidth: "1.7",
					strokeLinecap: "round"
				})]
			});
		}
		/** 标题栏的"新版本"按钮。点击后由宿主打开它已有的更新窗口。 */
		function UpdateButton() {
			return /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("button", {
				type: "button",
				className: "dshDesktopUpdateButton",
				title: "有新版本可用，点击查看并更新",
				onPointerDown: (event) => {
					event.preventDefault();
					event.stopPropagation();
				},
				onClick: () => postDesktopMessage("dsh-desktop:check-updates"),
				children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)(UpdateArrowIcon, {}), "新版本"]
			});
		}
		function AdvancedFrame({ layout, platform, renderSlot, useSessions }) {
			const subscribeLayout = (0, react.useCallback)((listener) => layout.subscribe(listener), [layout]);
			const readLayout = (0, react.useCallback)(() => layout.getSnapshot(), [layout]);
			const panels = (0, react.useSyncExternalStore)(subscribeLayout, readLayout);
			const frameRef = (0, react.useRef)(null);
			const [viewport, setViewport] = (0, react.useState)(() => window.innerWidth);
			const updateAvailable = useUpdateAvailable();
			const detailsSession = useSessions((state) => {
				const current = state.current;
				return current !== void 0 && state.byId[current]?.blank === false ? current : void 0;
			});
			(0, react.useEffect)(() => {
				const blockNavigation = (event) => {
					const types = event.dataTransfer?.types;
					if (types === void 0 || !types.includes("Files")) return;
					event.preventDefault();
				};
				document.addEventListener("dragover", blockNavigation, true);
				document.addEventListener("drop", blockNavigation, true);
				return () => {
					document.removeEventListener("dragover", blockNavigation, true);
					document.removeEventListener("drop", blockNavigation, true);
				};
			}, []);
			(0, react.useEffect)(() => {
				const element = frameRef.current;
				if (element === null) return;
				let raf = null;
				const observer = new ResizeObserver(() => {
					raf ??= requestAnimationFrame(() => {
						raf = null;
						const width = element.getBoundingClientRect().width;
						if (width > 0) setViewport(width);
					});
				});
				observer.observe(element);
				return () => {
					observer.disconnect();
					if (raf !== null) cancelAnimationFrame(raf);
				};
			}, []);
			const narrow = viewport < SIDEBAR_AUTO_COLLAPSE;
			(0, react.useEffect)(() => {
				layout.setNarrow(narrow);
			}, [layout, narrow]);
			const previousSession = (0, react.useRef)(detailsSession);
			(0, react.useLayoutEffect)(() => {
				if (detailsSession === void 0) return;
				if (previousSession.current !== void 0 && previousSession.current !== detailsSession) layout.closeDetails();
				previousSession.current = detailsSession;
			}, [detailsSession, layout]);
			const collapsed = narrow ? !panels.narrowExpanded : panels.sidebar === 0;
			const sidebarPreference = collapsed ? 0 : panels.sidebar === 0 ? 280 : panels.sidebar;
			const collapsedWidth = platform === "darwin" ? 90 : 56;
			const columns = computeDesktopColumns(viewport, sidebarPreference, detailsSession === void 0 ? 0 : panels.details, collapsedWidth);
			const normalRightbar = computeDesktopColumns(viewport, sidebarPreference, panels.details === 0 ? 360 : panels.details, collapsedWidth);
			const sidebarOwnerWidth = collapsed ? 56 : columns.sidebar;
			const columnsRef = (0, react.useRef)(columns);
			columnsRef.current = columns;
			const requestNativeDrag = (0, react.useCallback)((event) => {
				if (event.button !== 0) return;
				postDesktopMessage("dsh-desktop:start-dragging");
			}, []);
			const sidebarBase = (0, react.useRef)(0);
			const detailsBase = (0, react.useRef)(0);
			const [dragging, setDragging] = (0, react.useState)(false);
			const onDragEnd = (0, react.useCallback)(() => {
				setDragging(false);
			}, []);
			const onSidebarStart = (0, react.useCallback)(() => {
				sidebarBase.current = columnsRef.current.sidebar;
				setDragging(true);
			}, []);
			const onDetailsStart = (0, react.useCallback)(() => {
				detailsBase.current = columnsRef.current.details;
				setDragging(true);
			}, []);
			const onSidebarDrag = (0, react.useCallback)((dx) => {
				layout.setSidebar(sidebarBase.current + dx);
			}, [layout]);
			const onDetailsDrag = (0, react.useCallback)((dx) => {
				layout.setDetails(detailsBase.current - dx);
			}, [layout]);
			return /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
				ref: frameRef,
				className: "dshDesktopFrame",
				"data-desktop-platform": platform,
				"data-sidebar-collapsed": collapsed || void 0,
				"data-details-collapsed": columns.details === 0 || void 0,
				"data-dragging": dragging || void 0,
				style: { gridTemplateColumns: `${columns.sidebar}px minmax(0, 1fr) ${columns.details}px` },
				children: [
					platform === "darwin" && /* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
						className: "dshDesktopMacCaptionRow",
						onPointerDown: requestNativeDrag,
						children: updateAvailable && /* @__PURE__ */ (0, react_jsx_runtime.jsx)(UpdateButton, {})
					}),
					platform === "win32" && /* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
						className: "dshDesktopWindowsCaptionRow",
						onPointerDown: requestNativeDrag,
						children: updateAvailable && /* @__PURE__ */ (0, react_jsx_runtime.jsx)(UpdateButton, {})
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("aside", {
						className: "dshDesktopSidebarSurface",
						onPointerDown: (event) => {
							if (event.clientY > 32) return;
							if (platform === "darwin") {
								if (event.clientY <= 32 && event.clientX >= 80) requestNativeDrag(event);
								return;
							}
							if (platform === "win32") requestNativeDrag(event);
						},
						children: /* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
							className: "dshDesktopUpstreamSidebar",
							children: renderSlot("sidebar", {
								collapsed,
								width: sidebarOwnerWidth
							})
						})
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("main", {
						className: "dshDesktopConversationSurface",
						children: renderSlot("main", {}, { entryKey: panels.activePanelId ?? "conversation" })
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("aside", {
						className: "dshDesktopDetailsSurface",
						children: renderSlot("rightbar", {
							width: normalRightbar.details,
							viewportWidth: viewport,
							canShow: normalRightbar.details > 0
						})
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
						className: "dshDesktopOverlay",
						"data-shell-overlay": true,
						children: renderSlot("shell.overlay", {})
					}),
					!collapsed && /* @__PURE__ */ (0, react_jsx_runtime.jsx)(ResizeHandle, {
						side: "sidebar",
						left: columns.sidebar,
						onStart: onSidebarStart,
						onDrag: onSidebarDrag,
						onEnd: onDragEnd
					}),
					columns.details > 0 && /* @__PURE__ */ (0, react_jsx_runtime.jsx)(ResizeHandle, {
						side: "details",
						left: viewport - columns.details,
						onStart: onDetailsStart,
						onDrag: onDetailsDrag,
						onEnd: onDragEnd
					})
				]
			});
		}
		function ResizeHandle(props) {
			const [dragging, setDragging] = (0, react.useState)(false);
			const origin = (0, react.useRef)(0);
			const latest = (0, react.useRef)(0);
			const frame = (0, react.useRef)(null);
			const callbacks = (0, react.useRef)({
				onStart: props.onStart,
				onDrag: props.onDrag,
				onEnd: props.onEnd
			});
			callbacks.current = {
				onStart: props.onStart,
				onDrag: props.onDrag,
				onEnd: props.onEnd
			};
			const onPointerDown = (0, react.useCallback)((event) => {
				event.preventDefault();
				event.currentTarget.setPointerCapture(event.pointerId);
				origin.current = event.clientX;
				latest.current = event.clientX;
				callbacks.current.onStart();
				setDragging(true);
			}, []);
			const onPointerMove = (0, react.useCallback)((event) => {
				if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
				latest.current = event.clientX;
				frame.current ??= requestAnimationFrame(() => {
					frame.current = null;
					callbacks.current.onDrag(latest.current - origin.current);
				});
			}, []);
			const onPointerUp = (0, react.useCallback)((event) => {
				if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
				event.currentTarget.releasePointerCapture(event.pointerId);
				if (frame.current !== null) {
					cancelAnimationFrame(frame.current);
					frame.current = null;
				}
				callbacks.current.onDrag(latest.current - origin.current);
				setDragging(false);
				callbacks.current.onEnd();
			}, []);
			return /* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
				className: "dshDesktopResizeHandle",
				"data-side": props.side,
				"data-dragging": dragging || void 0,
				style: { left: props.left },
				onPointerDown,
				onPointerMove,
				onPointerUp
			});
		}
		//#endregion
		//#region src/client/desktop-bridge.ts
		const REQUEST_TYPE = "dsh-desktop:request";
		const RESPONSE_TYPE = "dsh-desktop:response";
		/** 宿主没响应时的放弃时限。设置读写都是本地调用，正常在毫秒级完成。 */
		const TIMEOUT_MS = 8e3;
		const pending = /* @__PURE__ */ new Map();
		let listening = false;
		let counter = 0;
		/** 宿主的响应只可能来自父窗口，其余来源一律忽略。 */
		function onMessage(event) {
			if (event.source !== window.parent) return;
			const message = event.data;
			if (message === null || typeof message !== "object") return;
			if (message.type !== RESPONSE_TYPE) return;
			const { id, ok, data, error } = message;
			if (typeof id !== "string") return;
			const entry = pending.get(id);
			if (entry === void 0) return;
			pending.delete(id);
			clearTimeout(entry.timer);
			if (ok === true) entry.resolve(data);
			else entry.reject(new Error(typeof error === "string" ? error : "宿主未返回结果"));
		}
		function ensureListening() {
			if (listening) return;
			window.addEventListener("message", onMessage);
			listening = true;
		}
		/**
		* 向宿主发一个请求并等待结果。
		*
		* @param command - 宿主侧的命令名。
		* @param payload - 命令参数，会原样转交给对应的 Tauri command。
		* @returns 宿主返回的数据。
		* @throws 宿主报错、超时，或当前不在 iframe 里运行时。
		*/
		function requestDesktop(command, payload) {
			if (window.parent === window) return Promise.reject(/* @__PURE__ */ new Error("未在桌面客户端中运行"));
			ensureListening();
			counter += 1;
			const id = `${Date.now().toString(36)}-${counter}`;
			return new Promise((resolve, reject) => {
				const timer = setTimeout(() => {
					pending.delete(id);
					reject(/* @__PURE__ */ new Error("桌面客户端未响应，请稍后重试"));
				}, TIMEOUT_MS);
				pending.set(id, {
					resolve,
					reject,
					timer
				});
				window.parent.postMessage({
					type: REQUEST_TYPE,
					id,
					command,
					payload
				}, "*");
			});
		}
		/**
		* 请求宿主弹出它已有的对话框。
		*
		* 关于、检查更新、反馈这三个窗口宿主早就实现了（含版本对比、下载进度、
		* 日志采集），这里只发通知让它显示，不在 DSH 侧重写一遍。
		*/
		function openHostDialog(dialog) {
			if (window.parent === window) return;
			window.parent.postMessage({ type: `dsh-desktop:open-${dialog}` }, "*");
		}
		//#endregion
		//#region \0dsh-css:/Users/llh928737095/Documents/AI-Agent/Projects/DeepSeek-Harness-Desktop/dsh-desktop-shell/src/client/./DesktopSettings.module.css.mjs
		const css = ".HapLFW_root{flex-direction:column;gap:12px;max-width:720px;display:flex;container-type:inline-size}.HapLFW_title{color:var(--dsw-alias-label-primary);margin:0;font-size:16px;font-weight:500;line-height:24px}.HapLFW_intro{color:var(--dsw-alias-label-tertiary);margin:0;font-size:13px;line-height:1.5}.HapLFW_group{border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-layer-3);border-radius:12px;flex-direction:column;padding:2px 16px;display:flex}.HapLFW_row+.HapLFW_row{border-top:1px solid var(--dsw-alias-border-l2)}.HapLFW_row{align-items:center;gap:12px;padding:12px 0;display:flex}.HapLFW_rowText{flex-direction:column;flex:1;gap:4px;min-width:0;display:flex}.HapLFW_label{color:var(--dsw-alias-label-primary);align-items:center;gap:6px;font-size:13px;line-height:20px;display:flex}.HapLFW_hint{color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:1.5}.HapLFW_help{color:var(--dsw-alias-label-tertiary);cursor:help;background:0 0;border:0;align-items:center;padding:0;display:inline-flex}.HapLFW_help:focus-visible{outline:2px solid var(--dsw-alias-brand-primary);outline-offset:2px;border-radius:99px}.HapLFW_switch{border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-layer-2);cursor:pointer;border-radius:99px;flex-shrink:0;width:38px;height:22px;padding:0;transition:background .15s,border-color .15s;position:relative}.HapLFW_switchOn{background:var(--dsw-alias-brand-primary);border-color:var(--dsw-alias-brand-primary)}.HapLFW_switchKnob{background:#fff;border-radius:99px;width:16px;height:16px;transition:left .15s,background .15s;position:absolute;top:2px;left:2px;box-shadow:0 1px 2px #00000040}body[data-ds-dark-theme] .HapLFW_switchKnob{background:var(--dsw-static-neutral-bluish-850);box-shadow:0 1px 2px #00000059}.HapLFW_switchOn .HapLFW_switchKnob{left:18px}.HapLFW_switch:disabled{opacity:.5;cursor:default}.HapLFW_switch:focus-visible{outline:2px solid var(--dsw-alias-brand-primary);outline-offset:2px}.HapLFW_status{grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:10px;display:grid}@container (width<=420px){.HapLFW_status{grid-template-columns:1fr}}.HapLFW_statusItem{border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-alias-bg-layer-3);border-radius:10px;flex-direction:column;gap:4px;padding:12px 14px;display:flex}.HapLFW_statusLabel{color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:1.5}.HapLFW_statusValue{color:var(--dsw-alias-label-primary);align-items:center;gap:6px;font-size:13px;line-height:20px;display:flex}.HapLFW_error{background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-state-error-primary);border-radius:8px;align-items:center;gap:8px;padding:10px 12px;font-size:12px;line-height:1.5;display:flex}.HapLFW_errorAction{color:inherit;font:inherit;cursor:pointer;background:0 0;border:1px solid;border-radius:6px;flex-shrink:0;margin-left:auto;padding:4px 10px;font-size:12px;line-height:18px}.HapLFW_errorAction:hover{background:var(--dsw-alias-bg-layer-3)}.HapLFW_errorAction:focus-visible{outline:2px solid var(--dsw-alias-brand-primary);outline-offset:2px}.HapLFW_loading{color:var(--dsw-alias-label-tertiary);font-size:13px;line-height:20px}.HapLFW_actions{justify-content:center;gap:8px;display:flex}.HapLFW_linkButton{border:1px solid var(--dsw-alias-border-l2);color:var(--dsw-alias-label-primary);font:inherit;cursor:pointer;background:0 0;border-radius:8px;padding:6px 14px;font-size:13px;line-height:20px;transition:background .15s}.HapLFW_linkButton:hover:not(:disabled){background:var(--dsw-alias-bg-layer-2)}.HapLFW_linkButton:disabled{opacity:.5;cursor:default}.HapLFW_about{color:var(--dsw-alias-label-tertiary);text-align:center;margin:0;font-size:12px;line-height:1.5}";
		const tagId = "dsh-desktop-shell/DesktopSettings.module.css";
		if (typeof document !== "undefined" && document.querySelector("style[data-plugin-css=" + JSON.stringify(tagId) + "]") === null) {
			const tag = document.createElement("style");
			tag.dataset.plugin = "dsh-desktop-shell";
			tag.dataset.pluginCss = tagId;
			tag.textContent = css;
			document.head.appendChild(tag);
		}
		var DesktopSettings_module_css_default = {
			"linkButton": "HapLFW_linkButton",
			"help": "HapLFW_help",
			"statusLabel": "HapLFW_statusLabel",
			"group": "HapLFW_group",
			"switch": "HapLFW_switch",
			"actions": "HapLFW_actions",
			"loading": "HapLFW_loading",
			"hint": "HapLFW_hint",
			"error": "HapLFW_error",
			"status": "HapLFW_status",
			"root": "HapLFW_root",
			"about": "HapLFW_about",
			"statusItem": "HapLFW_statusItem",
			"errorAction": "HapLFW_errorAction",
			"switchOn": "HapLFW_switchOn",
			"row": "HapLFW_row",
			"switchKnob": "HapLFW_switchKnob",
			"label": "HapLFW_label",
			"rowText": "HapLFW_rowText",
			"intro": "HapLFW_intro",
			"statusValue": "HapLFW_statusValue",
			"title": "HapLFW_title"
		};
		//#endregion
		//#region src/client/DesktopSettings.tsx
		/** 本分区用到的 primitives 导出，注册前逐个确认存在。 */
		const REQUIRED_PRIMITIVES = [
			"Tooltip",
			"IconQuestionOutline14",
			"IconWarningOutline16",
			"StateDot"
		];
		/**
		* 五个开关，按用户关心的优先级排序：
		* 更新最常改，其次是启动方式，再是运行期行为，最后是退出行为。
		*/
		const SWITCHES = [
			{
				key: "autoCheckUpdates",
				label: "自动检查更新",
				hint: "每隔 6 小时检查客户端和内核更新，发现新版本时提醒你。"
			},
			{
				key: "autoLaunch",
				label: "开机自启",
				hint: "系统启动时自动运行 DeepSeek Harness，适合频繁使用的场景。"
			},
			{
				key: "desktopNotifications",
				label: "桌面通知",
				hint: "任务完成、任务失败和后台任务状态会以系统通知提醒；不会显示会话正文。"
			},
			{
				key: "preventSleep",
				label: "防止系统休眠",
				hint: "运行期间阻止系统休眠，长时间任务不会被打断；屏幕仍可正常关闭。"
			},
			{
				key: "confirmExit",
				label: "关闭窗口时退出程序",
				hint: "开启后点关闭按钮会确认并完全退出；关闭时点关闭仅隐藏窗口，服务继续在后台运行。"
			}
		];
		function HelpMarker({ text }) {
			return /* @__PURE__ */ (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.Tooltip, {
				label: text,
				side: "top",
				delayMs: 150,
				maxWidth: 280,
				children: /* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", {
					className: DesktopSettings_module_css_default.help,
					tabIndex: 0,
					role: "img",
					"aria-label": text,
					children: /* @__PURE__ */ (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconQuestionOutline14, { size: 14 })
				})
			});
		}
		function Switch(props) {
			return /* @__PURE__ */ (0, react_jsx_runtime.jsx)("button", {
				type: "button",
				role: "switch",
				"aria-checked": props.checked,
				"aria-label": props.label,
				disabled: props.disabled,
				className: props.checked ? `${DesktopSettings_module_css_default.switch} ${DesktopSettings_module_css_default.switchOn}` : DesktopSettings_module_css_default.switch,
				onClick: props.onToggle,
				children: /* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", { className: DesktopSettings_module_css_default.switchKnob })
			});
		}
		/**
		* 客户端设置。
		*
		* 开关的真实状态由宿主持有——开机自启和防休眠对应真实的系统调用，
		* 所以每次写入后都重新读取一遍，避免界面显示的状态与系统实际状态不一致。
		*/
		function DesktopSettings() {
			const [values, setValues] = (0, react.useState)(null);
			const [state, setState] = (0, react.useState)(null);
			const [error, setError] = (0, react.useState)(null);
			const [showNotificationFix, setShowNotificationFix] = (0, react.useState)(false);
			const [busy, setBusy] = (0, react.useState)(null);
			const [clientVersion, setClientVersion] = (0, react.useState)(null);
			const load = (0, react.useCallback)(async () => {
				try {
					const [settings, runtime] = await Promise.all([requestDesktop("get-settings"), requestDesktop("get-state")]);
					setValues(settings);
					setState(runtime);
					setError(null);
				} catch (cause) {
					setError(cause instanceof Error ? cause.message : String(cause));
				}
			}, []);
			(0, react.useEffect)(() => {
				load();
			}, [load]);
			(0, react.useEffect)(() => {
				requestDesktop("check-client-update").then((info) => {
					setClientVersion(info.current_version);
				}).catch(() => {});
			}, []);
			const toggle = (0, react.useCallback)(async (key) => {
				if (values === null || busy !== null) return;
				const next = !values[key];
				setBusy(key);
				setError(null);
				setShowNotificationFix(false);
				setValues({
					...values,
					[key]: next
				});
				try {
					await requestDesktop("set-settings", { [key]: next });
					const fresh = await requestDesktop("get-settings");
					setValues(fresh);
				} catch (cause) {
					setError(cause instanceof Error ? cause.message : String(cause));
					load();
					setBusy(null);
					return;
				}
				if (key === "desktopNotifications" && next) try {
					if (!await requestDesktop("send-test-notification")) {
						setError("设置已保存，但系统没有显示测试通知。");
						setShowNotificationFix(true);
					}
				} catch (cause) {
					setError(cause instanceof Error ? cause.message : String(cause));
					setShowNotificationFix(true);
				}
				setBusy(null);
			}, [
				busy,
				load,
				values
			]);
			return /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
				className: DesktopSettings_module_css_default.root,
				children: [
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("h2", {
						className: DesktopSettings_module_css_default.title,
						children: "客户端设置"
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("p", {
						className: DesktopSettings_module_css_default.intro,
						children: "DeepSeek Harness 桌面客户端的偏好与运行状态。"
					}),
					error !== null && /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
						className: DesktopSettings_module_css_default.error,
						role: "alert",
						children: [
							/* @__PURE__ */ (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.IconWarningOutline16, { size: 16 }),
							/* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", { children: error }),
							showNotificationFix && /* @__PURE__ */ (0, react_jsx_runtime.jsx)("button", {
								type: "button",
								className: DesktopSettings_module_css_default.errorAction,
								onClick: () => {
									requestDesktop("open-notification-settings").catch(() => {});
								},
								children: "打开系统通知设置"
							})
						]
					}),
					values === null ? /* @__PURE__ */ (0, react_jsx_runtime.jsx)("p", {
						className: DesktopSettings_module_css_default.loading,
						children: "加载中…"
					}) : /* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
						className: DesktopSettings_module_css_default.group,
						children: SWITCHES.map((spec) => /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
							className: DesktopSettings_module_css_default.row,
							children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)("div", {
								className: DesktopSettings_module_css_default.rowText,
								children: /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
									className: DesktopSettings_module_css_default.label,
									children: [spec.label, /* @__PURE__ */ (0, react_jsx_runtime.jsx)(HelpMarker, { text: spec.hint })]
								})
							}), /* @__PURE__ */ (0, react_jsx_runtime.jsx)(Switch, {
								checked: values[spec.key],
								disabled: busy !== null,
								label: spec.label,
								onToggle: () => {
									toggle(spec.key);
								}
							})]
						}, spec.key))
					}),
					state !== null && /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
						className: DesktopSettings_module_css_default.status,
						children: [
							/* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
								className: DesktopSettings_module_css_default.statusItem,
								children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", {
									className: DesktopSettings_module_css_default.statusLabel,
									children: "客户端版本"
								}), /* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", {
									className: DesktopSettings_module_css_default.statusValue,
									children: clientVersion === null ? "—" : `v${clientVersion}`
								})]
							}),
							/* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
								className: DesktopSettings_module_css_default.statusItem,
								children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", {
									className: DesktopSettings_module_css_default.statusLabel,
									children: "内核版本"
								}), /* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", {
									className: DesktopSettings_module_css_default.statusValue,
									children: state.installedVersion === null ? "未安装" : `v${state.installedVersion}`
								})]
							}),
							/* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
								className: DesktopSettings_module_css_default.statusItem,
								children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)("span", {
									className: DesktopSettings_module_css_default.statusLabel,
									children: "服务状态"
								}), /* @__PURE__ */ (0, react_jsx_runtime.jsxs)("span", {
									className: DesktopSettings_module_css_default.statusValue,
									children: [/* @__PURE__ */ (0, react_jsx_runtime.jsx)(_deepseek_ai_dsh_client_ui_primitives.StateDot, {
										state: state.running ? "done" : "error",
										size: 8
									}), state.running ? state.port === null ? "运行中" : `运行中 · 端口 ${state.port}` : "未运行"]
								})]
							})
						]
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsxs)("div", {
						className: DesktopSettings_module_css_default.actions,
						children: [
							/* @__PURE__ */ (0, react_jsx_runtime.jsx)("button", {
								type: "button",
								className: DesktopSettings_module_css_default.linkButton,
								onClick: () => {
									openHostDialog("about");
								},
								children: "关于"
							}),
							/* @__PURE__ */ (0, react_jsx_runtime.jsx)("button", {
								type: "button",
								className: DesktopSettings_module_css_default.linkButton,
								onClick: () => {
									openHostDialog("check-updates");
								},
								children: "检查更新"
							}),
							/* @__PURE__ */ (0, react_jsx_runtime.jsx)("button", {
								type: "button",
								className: DesktopSettings_module_css_default.linkButton,
								onClick: () => {
									openHostDialog("feedback");
								},
								children: "反馈问题"
							})
						]
					}),
					/* @__PURE__ */ (0, react_jsx_runtime.jsx)("p", {
						className: DesktopSettings_module_css_default.about,
						children: "DeepSeek Harness 桌面客户端 · Copyright © 2026 INNOTECH"
					})
				]
			});
		}
		//#endregion
		//#region src/client/layout-service.ts
		/**
		* Provide the advanced layout service for one plugin-fiber lifetime.
		*
		* Also installs the official `panelInfo` root hook. WorkspaceBrowser /
		* SessionTree / FlatList / SearchResults all call `usePanelInfo(...)`
		* unconditionally; missing it throws while rendering the session tree, which
		* looks like an empty history list with the New Session chrome still visible.
		*
		* @param ctx - active browser Cordis context.
		* @param layout - desktop-owned layout implementation.
		* @returns disposer for the service registration.
		*/
		function provideDesktopLayout(ctx, layout) {
			const disposeService = ctx.reflect.provide("layout", layout);
			const disposePanelInfo = ctx.slots.provideRoot({ hooks: { panelInfo: {
				getSnapshot: () => layout.getPanelInfo(),
				subscribe: (listener) => layout.subscribe(listener)
			} } });
			return () => {
				layout.dispose();
				disposePanelInfo();
				disposeService();
			};
		}
		//#endregion
		//#region src/client/styles.ts
		/** Advanced-shell stylesheet kept as a plain string so the package client bundle stays self-contained. */
		const ADVANCED_STYLES = `
html, body, #root { width: 100%; height: 100%; }
body[data-dsh-desktop-mode="advanced"] { margin: 0; background: transparent !important; }
.dshDesktopFrame { position: relative; display: grid; grid-template-rows: 100%; width: 100%; height: 100%; overflow: hidden; background: var(--dsw-alias-bg-base); transition: grid-template-columns var(--ds-transition-duration-slow) var(--ds-ease-in-out); }
.dshDesktopSidebarSurface { --dsw-specific-sidebar-fill: transparent; position: relative; grid-column: 1; grid-row: 1; min-width: 0; overflow: hidden; background: transparent; border-right: 1px solid var(--dsw-alias-border-l1); }
.dshDesktopUpstreamSidebar { box-sizing: border-box; width: 100%; height: 100%; }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopUpstreamSidebar { padding-top: 28px; -webkit-app-region: no-drag; }
.dshDesktopFrame[data-desktop-platform="darwin"][data-sidebar-collapsed] .dshDesktopUpstreamSidebar { width: 56px; margin: 0 auto; }
.dshDesktopFrame[data-desktop-platform="darwin"] { grid-template-rows: 36px minmax(0, 1fr); }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopSidebarSurface { grid-row: 1 / -1; -webkit-app-region: no-drag; }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopConversationSurface,
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopDetailsSurface { grid-row: 2; }
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopSidebarSurface::before { content: ""; position: absolute; z-index: 100; top: 0; right: 0; left: 80px; height: 32px; user-select: none; -webkit-app-region: drag; }
.dshDesktopMacCaptionRow { position: relative; z-index: 100; grid-column: 2 / -1; grid-row: 1; min-width: 0; background: var(--dsw-alias-bg-base); }
.dshDesktopMacCaptionRow::before { content: ""; position: absolute; inset: 0; user-select: none; -webkit-app-region: drag; }
/* 按钮要压在拖动层之上，否则点击会被拖动区吃掉。 */
.dshDesktopMacCaptionRow .dshDesktopUpdateButton { z-index: 2; }
.dshDesktopConversationSurface { grid-column: 2; grid-row: 1; min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow: hidden; background: var(--dsw-alias-bg-base); }
/*
	  Official SidebarPanel is position:absolute; top/bottom/right:0 and slides
	  with translate(100%). Official AppFrame therefore gives the right column
	  position:relative; overflow:visible so those offsets are the column, not
	  the window, and so the dock-kit chrome (collapse / fullscreen) is not
	  clipped into the caption row. Frame overflow:hidden still clips the slide.
	*/
	.dshDesktopDetailsSurface { grid-column: 3; grid-row: 1; position: relative; min-width: 0; min-height: 0; overflow: visible; background: var(--dsw-alias-bg-base); border-left: 1px solid var(--dsw-alias-border-l2); }
.dshDesktopFrame[data-details-collapsed] .dshDesktopDetailsSurface { border-left: none; }
/*
  Windows 跟 macOS 同一套网格：会话/详情让出顶部 32px 给最小化/最大化/关闭，
  侧边栏通栏到窗口顶边、不加 padding。宿主的三个按钮画在 webview 物理右上角，
  正好落在这条预留带上，不再压住会话头部的日志下载 / 在本地打开 / 打开侧边栏。

  不要把侧边栏也放到第二行——整块界面会凭空矮一截，看起来不像同一个窗口。
*/
.dshDesktopFrame[data-desktop-platform="win32"] { grid-template-rows: 32px minmax(0, 1fr); background: var(--dsw-alias-bg-base); }
.dshDesktopFrame[data-desktop-platform="win32"] .dshDesktopSidebarSurface { grid-row: 1 / -1; }
.dshDesktopFrame[data-desktop-platform="win32"] .dshDesktopConversationSurface,
.dshDesktopFrame[data-desktop-platform="win32"] .dshDesktopDetailsSurface { grid-row: 2; }
.dshDesktopWindowsCaptionRow { position: relative; z-index: 100; grid-column: 2 / -1; grid-row: 1; min-width: 0; background: var(--dsw-alias-bg-base); user-select: none; }
.dshDesktopWindowsCaptionRow::before { content: ""; position: absolute; inset: 0 138px 0 0; pointer-events: auto; user-select: none; -webkit-app-region: drag; }
.dshDesktopUpdateButton { position: absolute; z-index: 2; top: 4px; right: 146px; display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 12px 0 10px; border: 0; border-radius: 999px; color: #fff; background: var(--dsw-alias-state-success-primary); font: 500 12px/1 inherit; letter-spacing: .2px; white-space: nowrap; cursor: pointer; pointer-events: auto; -webkit-app-region: no-drag !important; box-shadow: 0 1px 3px rgba(0, 0, 0, .16); }
.dshDesktopUpdateButton:hover { background: var(--dsw-alias-state-success-secondary); }
.dshDesktopUpdateButton:active { transform: translateY(.5px); }
.dshDesktopUpdateButton:focus-visible { outline: 2px solid var(--dsw-alias-state-success-primary); outline-offset: 2px; }
/* 到上边和右边的距离相等，按钮才像是贴着角摆放，而不是被挤在角上。 */
.dshDesktopFrame[data-desktop-platform="darwin"] .dshDesktopUpdateButton { top: 6px; right: 6px; }
.dshDesktopFrame[data-dragging] { transition: none; }
.dshDesktopOverlay { position: absolute; z-index: 1000; inset: 0; pointer-events: none; }
.dshDesktopOverlay > * { pointer-events: auto; }
.dshDesktopResizeHandle { position: absolute; z-index: 50; top: 0; bottom: 0; width: 8px; margin-left: -4px; cursor: col-resize; touch-action: none; -webkit-app-region: no-drag; transition: left var(--ds-transition-duration-slow) var(--ds-ease-in-out); }
.dshDesktopFrame[data-dragging] .dshDesktopResizeHandle { transition: none; }
.dshDesktopNoDrag, button, input, textarea, select, a, [contenteditable="true"], [role="button"], [role="checkbox"], [role="dialog"], [role="menu"], [role="menuitem"], [role="option"], [role="switch"], [role="tab"] { -webkit-app-region: no-drag !important; }
[role="dialog"], [aria-modal="true"] { -webkit-app-region: no-drag !important; }
html:has([aria-modal="true"]) .dshDesktopWindowsCaptionRow::before,
html:has([aria-modal="true"]) .dshDesktopMacCaptionRow::before,
html:has([aria-modal="true"]) .dshDesktopSidebarSurface,
html:has([aria-modal="true"]) .dshDesktopSidebarSurface::before { -webkit-app-region: no-drag !important; }
@media (prefers-reduced-motion: reduce) {
  .dshDesktopFrame,
  .dshDesktopResizeHandle { transition: none !important; }
}
`;
		/** Install and remove the advanced shell's global native-window styles. @returns the style disposer. */
		function installAdvancedStyles() {
			const style = document.createElement("style");
			style.dataset.plugin = "dsh-plugin-desktop";
			style.dataset.pluginCss = "dsh-plugin-desktop/advanced-shell";
			style.textContent = ADVANCED_STYLES;
			document.head.appendChild(style);
			return () => {
				style.remove();
			};
		}
		//#endregion
		//#region src/client/theme-presenter.ts
		const DARK_ATTRIBUTE = "data-ds-dark-theme";
		/** Projects the resolved theme service snapshot onto the desktop document. */
		var DesktopThemePresenter = class {
			appliedTokens = [];
			themeColorMeta = document.createElement("meta");
			reportedDark;
			constructor() {
				this.themeColorMeta.name = "theme-color";
			}
			/** @param snapshot - current resolved palette and token overrides. */
			apply(snapshot) {
				const scheme = snapshot.active.colorScheme;
				document.documentElement.style.colorScheme = scheme;
				document.body.style.colorScheme = scheme;
				if (scheme === "dark") document.body.setAttribute(DARK_ATTRIBUTE, "");
				else document.body.removeAttribute(DARK_ATTRIBUTE);
				for (const name of this.appliedTokens) document.body.style.removeProperty(name);
				this.appliedTokens = [];
				for (const [name, value] of Object.entries(snapshot.active.tokens)) {
					document.body.style.setProperty(name, value);
					this.appliedTokens.push(name);
				}
				this.themeColorMeta.content = getComputedStyle(document.body).backgroundColor;
				if (!this.themeColorMeta.isConnected) document.head.appendChild(this.themeColorMeta);
				this.reportToHost(scheme === "dark");
			}
			/**
			* Tell the launcher which scheme is active.
			*
			* The launcher paints the Windows caption controls itself, outside this
			* document — without this signal it can only follow the OS preference, so
			* switching the theme inside DSH would leave dark glyphs on a dark caption.
			*/
			reportToHost(dark) {
				if (this.reportedDark === dark) return;
				this.reportedDark = dark;
				if (window.parent === window) return;
				window.parent.postMessage({
					type: "theme-change",
					dark
				}, "*");
			}
			/** Remove only DOM state owned by this presenter. */
			dispose() {
				document.documentElement.style.removeProperty("color-scheme");
				document.body.style.removeProperty("color-scheme");
				document.body.removeAttribute(DARK_ATTRIBUTE);
				for (const name of this.appliedTokens) document.body.style.removeProperty(name);
				this.appliedTokens = [];
				this.themeColorMeta.remove();
				this.reportedDark = void 0;
			}
		};
		//#endregion
		//#region src/client/advanced-shell.ts
		/**
		* Provide the advanced layout service and own the desktop root slot.
		* @param ctx - active browser Cordis context.
		* @param environment - validated mode and platform marker.
		*/
		function applyAdvancedShell(ctx, environment) {
			if (environment.mode !== "advanced") throw new Error(`dsh-plugin-desktop: advanced shell received mode ${JSON.stringify(environment.mode)}`);
			const desktopLayout = new DesktopLayoutState();
			ctx.effect(() => provideDesktopLayout(ctx, desktopLayout), "desktop: layout service");
			ctx.effect(() => {
				document.body.dataset.dshDesktopMode = "advanced";
				document.body.dataset.dshDesktopPlatform = environment.platform;
				const removeStyles = installAdvancedStyles();
				return () => {
					removeStyles();
					delete document.body.dataset.dshDesktopMode;
					delete document.body.dataset.dshDesktopPlatform;
				};
			}, "desktop: advanced shell styles");
			ctx.effect(() => {
				const presenter = new DesktopThemePresenter();
				presenter.apply(ctx.theme.getTheme());
				const off = ctx.events.on("theme/change", (snapshot) => {
					presenter.apply(snapshot);
				});
				return () => {
					off();
					presenter.dispose();
				};
			}, "desktop: theme presenter");
			ctx.effect(() => {
				const disposeRoot = ctx.slots.register({
					name: "root",
					children: {
						"sidebar": {
							kind: "single",
							scope: "root"
						},
						"main": {
							kind: "keyed",
							scope: "root"
						},
						"rightbar": {
							kind: "single",
							scope: "root"
						},
						"shell.overlay": {
							kind: "list",
							scope: "root"
						}
					},
					inject: () => ({
						layout: desktopLayout,
						platform: environment.platform
					})
				}, AdvancedFrame);
				const retainMainPanels = () => {
					desktopLayout.retainMainPanels(ctx.slots.entries("main").flatMap((entry) => {
						const key = entry.options.key;
						return typeof key === "string" ? [key] : [];
					}));
				};
				const disposePanels = ctx.slots.subscribe("main", retainMainPanels);
				retainMainPanels();
				return () => {
					disposePanels();
					disposeRoot();
				};
			}, "desktop: advanced root slot");
			registerDesktopSettings(ctx);
		}
		/**
		* 把桌面客户端设置注册成 DSH 原生设置里的一个分区。
		*
		* `settings.section` 只在设置外壳挂载期间存在，所以必须用 `slots.inject`
		* 包一层，不能直接 register。order 取 50 排在内置分区之后
		* （通用 0、模型 10、插件 15、智能体预设 20、插件市场 40）。
		*
		* primitives 由宿主的固定模块表提供，不在依赖图里。旧版宿主会把新导出解析成
		* `undefined`，渲染时抛错会让整个设置对话框白屏——所以先确认用到的导出都在，
		* 缺失时放弃注册，只丢掉本分区。
		*/
		function registerDesktopSettings(ctx) {
			const missing = REQUIRED_PRIMITIVES.filter((name) => _deepseek_ai_dsh_client_ui_primitives[name] === void 0);
			if (missing.length > 0) {
				console.warn(`[dsh-desktop-shell] 宿主 ui-primitives 缺少 ${missing.join("、")}，桌面设置分区已跳过`);
				return;
			}
			ctx.effect(() => ctx.slots.inject("settings.section", () => ctx.slots.register({
				name: "settings.section",
				id: "desktop",
				order: 50,
				label: () => "客户端设置"
			}, DesktopSettings)), "desktop: settings section");
		}
		//#endregion
		//#region src/client/environment.ts
		function desktopEnvironment() {
			return {
				mode: "advanced",
				platform: /Mac/i.test(navigator.platform) ? "darwin" : /Win/i.test(navigator.platform) ? "win32" : "linux"
			};
		}
		//#endregion
		//#region src/client/open-in-app-bridge.ts
		const OPEN_IN_APP_OPEN_PATH = "/open-in-app/open";
		/**
		* Windows 上官方「在本地打开」会 POST `/open-in-app/open`，宿主再用隐藏的
		* Node 进程跑 `powershell Invoke-Item`。那个进程带着 CREATE_NO_WINDOW，
		* Explorer 经常不出现。这里只拦截资源管理器这一条，改走桌面宿主的 opener。
		*
		* 不能在 host 侧再注册同一条 exact 路由——webServer 遇到重复 (kind, path) 会抛错。
		* GET 应用列表和其它应用的 argv 启动仍走官方。
		*/
		function installWindowsOpenInAppBridge(platform) {
			if (platform !== "win32") return () => {};
			const original = window.fetch.bind(window);
			window.fetch = async (input, init) => {
				const intercepted = tryInterceptExplorerOpen(input, init);
				if (intercepted !== null) return intercepted;
				return original(input, init);
			};
			return () => {
				window.fetch = original;
			};
		}
		function tryInterceptExplorerOpen(input, init) {
			if (!isOpenInAppPost(input, init)) return null;
			const body = readJsonBody(init?.body);
			if (body === null) return null;
			if (body.app !== "explorer" || typeof body.path !== "string") return null;
			return openExplorer(body.path);
		}
		function isOpenInAppPost(input, init) {
			if ((init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase() !== "POST") return false;
			return pathnameOf(input) === OPEN_IN_APP_OPEN_PATH;
		}
		function pathnameOf(input) {
			try {
				if (typeof input === "string") return new URL(input, window.location.origin).pathname;
				if (input instanceof URL) return input.pathname;
				return new URL(input.url, window.location.origin).pathname;
			} catch {
				return "";
			}
		}
		function readJsonBody(body) {
			if (typeof body !== "string") return null;
			try {
				const parsed = JSON.parse(body);
				if (parsed === null || typeof parsed !== "object") return null;
				return parsed;
			} catch {
				return null;
			}
		}
		async function openExplorer(path) {
			try {
				await requestDesktop("open-path", { path });
				return jsonResponse({ ok: true }, 200);
			} catch (error) {
				return jsonResponse({
					code: "launch-failed",
					error: error instanceof Error ? error.message : String(error)
				}, 502);
			}
		}
		function jsonResponse(body, status) {
			return new Response(JSON.stringify(body), {
				status,
				headers: { "content-type": "application/json" }
			});
		}
		//#endregion
		//#region src/client/index.ts
		const inject = [
			"slots",
			"sessions",
			"theme",
			"locale"
		];
		function apply(ctx) {
			const environment = desktopEnvironment();
			ctx.effect(() => installWindowsOpenInAppBridge(environment.platform), "desktop: windows open-in-app bridge");
			try {
				applyAdvancedShell(ctx, environment);
			} catch (error) {
				console.error("[dsh-desktop-shell] advanced layout unavailable; keeping official layout", error);
			}
		}
		//#endregion
		exports.apply = apply;
		exports.inject = inject;
		return module.exports;
	}
});

//# sourceMappingURL=client.js.map