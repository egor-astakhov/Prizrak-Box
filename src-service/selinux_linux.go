//go:build linux

package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// SELinux (Fedora, RHEL и производные в режиме enforcing).
//
// Пакеты кладут px и px-service в /usr/lib/... (Electron: /usr/lib/Prizrak-Box/
// resources, Wails: /usr/lib/prizrak-box), а файлам там политика назначает тип
// lib_t. Для lib_t нет перехода домена, поэтому systemd запускает сервис прямо в
// своём домене init_t, и px, порождённый сервисом, остаётся в нём же. init_t не
// может писать в домашний каталог пользователя (user_home_t): px сразу падает с
// "open ~/Prizrak-Box-V3/pid/px-server.pid: permission denied", а приложение,
// ожидающее от него порт, не открывает окно (проверено на Fedora 43, enforcing).
// Исполняемым файлам нужен тип bin_t — тогда systemd запускает их в
// unconfined_service_t, как любой сторонний сервис из /usr/bin.
//
// Правило добавляется через semanage fcontext, поэтому оно постоянное: его
// применяет и restorecon, и сам rpm при обновлении пакета (файлы новой версии
// сразу получают bin_t). Если semanage нет (на Fedora Workstation он есть —
// пакет policycoreutils-python-utils), метка ставится через chcon: этого
// хватает до следующего обновления пакета или полной перемаркировки ФС, а
// скрипты пакетов повторяют её после каждой установки.

// selinuxEnabled сообщает, включён ли SELinux (в любом режиме: в permissive
// отказы только пишутся в журнал, но метка понадобится при переходе в enforcing).
func selinuxEnabled() bool {
	if _, err := os.Stat("/sys/fs/selinux/enforce"); err == nil {
		return true
	}
	if path, err := exec.LookPath("selinuxenabled"); err == nil {
		return exec.Command(path).Run() == nil
	}
	return false
}

// selinuxServiceBinaries возвращает исполняемые файлы, которые запускаются от
// имени сервиса: сам px-service и px рядом с ним.
func selinuxServiceBinaries(execPath string) []string {
	paths := []string{execPath}
	px := filepath.Join(filepath.Dir(execPath), "px")
	if _, err := os.Stat(px); err == nil {
		paths = append(paths, px)
	}
	return paths
}

// labelServiceBinariesForSELinux назначает файлам сервиса тип bin_t. Ошибки не
// фатальны: на системе без SELinux или с нестандартной политикой установка
// продолжается как раньше, а причина остаётся в выводе.
func labelServiceBinariesForSELinux(execPath string) {
	if !selinuxEnabled() {
		return
	}
	semanage, semanageErr := exec.LookPath("semanage")
	for _, path := range selinuxServiceBinaries(execPath) {
		if semanageErr == nil {
			// -a падает, если правило для этого пути уже есть (повторная
			// установка) — тогда его изменяет -m.
			if runTool(semanage, "fcontext", "-a", "-t", "bin_t", path) == nil ||
				runTool(semanage, "fcontext", "-m", "-t", "bin_t", path) == nil {
				if err := runTool("restorecon", path); err == nil {
					fmt.Printf("SELinux: labelled %s as bin_t\n", path)
					continue
				}
			}
		}
		if err := runTool("chcon", "-t", "bin_t", path); err != nil {
			fmt.Fprintf(os.Stderr, "SELinux: could not label %s as bin_t: %v\n", path, err)
			continue
		}
		fmt.Printf("SELinux: labelled %s as bin_t (chcon, not persistent)\n", path)
	}
}

// removeSELinuxLabelRules удаляет правила, добавленные при установке. Сами
// файлы перемаркировывать не нужно: они либо удаляются вместе с пакетом, либо
// останутся с bin_t, что безвредно.
func removeSELinuxLabelRules(execPath string) {
	if !selinuxEnabled() {
		return
	}
	semanage, err := exec.LookPath("semanage")
	if err != nil {
		return
	}
	for _, path := range selinuxServiceBinaries(execPath) {
		_ = runTool(semanage, "fcontext", "-d", path)
	}
}

func runTool(name string, args ...string) error {
	out, err := exec.Command(name, args...).CombinedOutput()
	if err != nil {
		if msg := strings.TrimSpace(string(out)); msg != "" {
			return fmt.Errorf("%w: %s", err, msg)
		}
	}
	return err
}
