; NSIS 安装钩子。
;
; 只做一件事：删掉旧版留下的 dshdesk.exe。
;
; 1.0.1 把主程序改名成 DeepSeek Harness.exe（系统各处都要显示同一个名字），
; 但 NSIS 的升级安装只覆盖同名文件、不清理改名前的旧文件。结果安装目录里会
; 同时躺着两个可执行文件，旧的那个还会被残留的快捷方式指着——用户点开的是
; 上一版，而且系统弹权限请求时显示的仍是内部代号 dshdesk。

!macro NSIS_HOOK_PREINSTALL
  ; 卸载器会话里 $INSTDIR 也指向安装目录，同一段逻辑复用即可。
  Delete "$INSTDIR\dshdesk.exe"
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; 覆盖安装过程若重新解出旧文件（历史包里带过），这里再清一次。
  Delete "$INSTDIR\dshdesk.exe"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$INSTDIR\dshdesk.exe"
!macroend
