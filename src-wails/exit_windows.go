package main

import "golang.org/x/sys/windows"

// terminateAfterShutdown ends the process as soon as Wails has finished its
// shutdown (Options.PostShutdown): services are stopped (kvstore saved, px
// asked to exit), the tray icon is removed and the single-instance lock is
// released, so nothing of ours is left to run.
//
// Returning from main instead would end in ExitProcess, which runs the
// DLL_PROCESS_DETACH handlers of every module still loaded — including the
// in-process WebView2 host DLL, whose controller can still be alive at that
// point: Wails closes the window from an event listener goroutine, so its
// WM_CLOSE may not have been handled yet when the message loop exits. A
// teardown racing the detach is reported by users as a native
// "Fail Fast exception" dialog on quit (Windows 10). TerminateProcess skips the
// detach phase entirely; the WebView2 browser processes notice the host is gone
// and shut themselves down, exactly as they do after a normal exit.
func terminateAfterShutdown() {
	_ = windows.TerminateProcess(windows.CurrentProcess(), 0)
}
