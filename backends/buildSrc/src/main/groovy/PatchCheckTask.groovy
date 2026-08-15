// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

// Fails the build if a third_party submodule has patches which haven't been
// applied (i.e., it's still sitting at the pinned upstream commit), so a build
// can't silently come out of pristine sources. See patch.sh.
abstract class PatchCheckTask extends DefaultTask {
    PatchCheckTask() {
        // what's being checked is the submodule working trees, which gradle
        // has no way to snapshot
        outputs.upToDateWhen { false }
    }

    @Internal
    abstract RegularFileProperty getPatchScript()

    @Inject
    abstract ExecOperations getExecOperations()

    @TaskAction
    void check() {
        def script = patchScript.get().asFile
        if (!script.isFile()) {
            throw new GradleException("no patch script at ${script}")
        }

        // captured so the failure shows patch.sh's own message (which says how
        // to fix it) rather than gradle's generic exit status one
        def out = new ByteArrayOutputStream()
        def result = execOperations.exec {
            it.workingDir = script.parentFile
            it.commandLine = [script.absolutePath, "check"]
            it.standardOutput = out
            it.errorOutput = out
            it.ignoreExitValue = true
        }

        def text = out.toString("UTF-8").trim()
        if (result.exitValue != 0) {
            throw new GradleException(text.isEmpty()
                ? "${script.name} check failed with exit status ${result.exitValue}"
                : text)
        }
        if (!text.isEmpty()) {
            logger.lifecycle(text)
        }
    }
}
