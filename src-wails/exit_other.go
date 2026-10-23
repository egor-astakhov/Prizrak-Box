//go:build !windows

package main

// terminateAfterShutdown is Windows-only (see exit_windows.go); elsewhere the
// process exits normally once app.Run returns.
func terminateAfterShutdown() {}
