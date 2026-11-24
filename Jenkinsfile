pipeline {
    // Agents wrote the code this builds, so it runs where they already do: the agent on their own box.
    // Never on the agents that deploy the homelab. Jenkins enforces that per node, not this label.
    agent { label 'untrusted' }

    options {
        skipDefaultCheckout()
        // Per branch or pull request; a discarded build takes its archived quality.md with it. Jenkins never discards
        // the last successful build, which GIT_PREVIOUS_SUCCESSFUL_COMMIT names for the Gate stage.
        buildDiscarder(logRotator(numToKeepStr: '30'))
    }

    environment {
        UV_NO_PROGRESS = '1'
        // For the Test, UI, Gate and Package stages: crates are fetched into the workspace, which cleanWs empties, so
        // no build compiles sources an earlier build left behind. .gitignore ignores it, and .tools/ below, so the
        // Gate stage does not judge them as files the change adds.
        CARGO_HOME     = "${env.WORKSPACE}/.cargo-home"
        // Temporary files on the agent's mounted volume, beside the workspace, rather than in /tmp. The tests and the
        // gate build Rust in temporary directories and run what they build: the test fixtures' build scripts and test
        // binaries, and cargo-mutants on the gate's copy of a tree; and the Package stage's AppImage smoke test runs
        // the app extracted there. /tmp is in the container's writable layer, where the agent host's Falco reports
        // every program run from a file written there ("Drop and execute new binary in container", Critical): 109
        // mails in 15 minutes on 2026-11-03. The workspace's own target/ is on the volume and was never reported.
        // Python's tempfile takes /tmp, silently, for a TMPDIR that does not exist, so the Checkout stage creates it
        // first.
        TMPDIR         = "${env.WORKSPACE_TMP}/tmp"
        // Tools the Install stage downloads into the workspace (ci/install-tools.sh), first on PATH for every stage.
        PATH           = "${env.WORKSPACE}/.tools/bin:${env.PATH}"
        // The dev profile, which the Test stage's test build and clippy's checks inherit, without debug info. Debug
        // info for the GTK and Tauri crates took enough memory that rustc was OOM-killed with two builds on
        // agentbox0. ci/cargo-lock.sh now keeps two builds' cargo steps apart, except the Gate stage's clippy, which
        // the gate runs outside it; building without debug info
        // keeps each build's own peak lower. A failing test still prints its panic's file:line, which comes from
        // core::panic::Location, not from debug info. Set for the whole build, not the UI stage alone, so that the
        // Gate stage's clippy finds the dependencies the UI stage's cargo check checked fresh, and checks only the
        // shell's own crate outside the lock; a difference between the two stages' cargo environments, such as a
        // RUSTFLAGS one of them sets, would have it check them all again there.
        CARGO_PROFILE_DEV_DEBUG = '0'
        // The GitHub login Jenkins posts pull request statuses as, its GitHub App's "<slug>[bot]", which the agents
        // cannot post as. Empty: no build of main trusts a pull request build, and each judges itself.
        PR_STATUS_CREATOR = 'davidsiegl-jenkins[bot]'
    }

    stages {
        stage('Checkout') {
            steps {
                // What an earlier build of this workspace left in TMPDIR goes first: on the volume, unlike in the
                // container's /tmp, it would otherwise stay. The chmod is for a directory a killed test left without
                // write permission, such as engine/src/host/tests.rs's 0o500 one; a clear-out that still fails only
                // warns, since failing here would fail every later build of this workspace until someone cleaned the
                // volume by hand.
                sh '''
                    if [ -e "$TMPDIR" ]; then
                        { chmod -R u+rwX "$TMPDIR" && rm -rf "$TMPDIR"; } ||
                            echo "could not empty $TMPDIR; what an earlier build left there stays" >&2
                    fi
                    mkdir -p "$TMPDIR"
                '''
                script {
                    def checkedOut = checkout scm
                    // The Gate stage measures a build of main from here.
                    env.PREVIOUS_SUCCESSFUL_COMMIT = checkedOut.GIT_PREVIOUS_SUCCESSFUL_COMMIT ?: ''
                }
            }
        }

        // A build of main skips Test and Gate when a pull request build already judged its tree
        // (ci/judged-at-pr.sh). Whatever stops the script from saying so, the build judges the tree itself.
        stage('Judged at PR') {
            when { branch 'main' }
            steps {
                withCredentials([string(credentialsId: 'GitHub-Agents', variable: 'GH_TOKEN')]) {
                    script {
                        if (sh(returnStatus: true, script: 'ci/judged-at-pr.sh') == 0) {
                            env.JUDGED_AT_PR = 'true'
                        }
                    }
                }
            }
        }

        stage('Install') {
            steps {
                // The Gate stage's Python and its Markdown linter, pyproject.toml's dev group: octoflow has no Python
                // of its own. --frozen, here and in the Gate stage, installs what uv.lock holds without asking whether
                // it still matches pyproject.toml, so `uv lock --check` first fails a build whose uv.lock was not
                // resolved from pyproject.toml's current requirements, rather than judging with tools that may not
                // meet them. It needs uv 0.12 or later, as pyproject.toml's required-version says.
                sh 'uv lock --check'
                sh 'uv sync --frozen'
                // The Test stage's TestClippy and the Gate stage's gate run the real clippy, the Test stage's
                // cargo-llvm-cov reads its coverage data with llvm-tools' llvm-profdata and llvm-cov, which must be the
                // toolchain's own LLVM, and its ci/rustfmt.sh runs rustfmt. All are rustup components the agent image
                // provides: its rustup home, /usr/local/rustup, is root's, so a build cannot add to it (PR-163's builds
                // 1-3 failed adding llvm-tools-preview: "component download failed ... No such file or directory").
                // This step names a missing one here rather than letting a later stage fail on it. rustup reads
                // rust-toolchain.toml here, so it lists the components of the toolchain that file pins, which must be
                // the image's too: rustup cannot install another one either, and says so as "toolchain ... is not
                // installed". RUSTUP_AUTO_INSTALL=0 keeps rustup from installing a component missing from that
                // toolchain before it lists, which fails in the root-owned home with only "Permission denied"
                // (PR-495's build 1), so the listing works and the loop below names the component.
                sh '''
                    installed=$(RUSTUP_AUTO_INSTALL=0 rustup component list --installed)
                    for component in clippy llvm-tools rustfmt; do
                        printf '%s\\n' "$installed" | grep -q "^$component-" || {
                            echo "rustup's $component component is not on the agent; its image must add it" >&2
                            exit 1
                        }
                    done
                '''
                sh 'ci/install-tools.sh "$WORKSPACE/.tools/bin"'
            }
        }

        stage('Test') {
            when { expression { return env.JUDGED_AT_PR != 'true' } }
            steps {
                // A failing test does not stop the build here: the Gate stage judges it with everything else, so its
                // report lists failing tests and findings together.
                //
                // The Rust tests of every member of the top-level Cargo workspace (Cargo.toml), the engine's, its e2e
                // scenarios' and docs check's, the desktop shell's and xtask's (which build the eval tasks' toy crates,
                // and the engine into target/xtask-engine), with line coverage, for the Gate stage likewise
                // (ci/rust-tests.sh, which says what it writes and when it fails). Once the tests are built,
                // --ignore-run-fail turns any failure of nextest's run into a warning (run_nextest in cargo-llvm-cov
                // 0.9.1's src/main.rs), also one that runs no test, such as a test binary that cannot list its tests;
                // so the JUnit report is removed first and must be there after. The lock as in the UI stage.
                //
                // OCTOFLOW_TEST_SEED is the run seed of the engine's generated-input tests (engine/src/test_support.rs
                // says what it changes), a fresh one each build. It is echoed here, whatever the tests do, as the
                // assignment that runs a build's cases again.
                //
                // OCTOFLOW_REAL_GATE=1 runs the e2e scenario real_gate on the octoflow-gate release the engine has
                // built in, downloaded once into OCTOFLOW_GATE_CACHE, beside the workspace, which builds keep
                // (engine/e2e/tests/e2e/real_gate.rs).
                sh '''
                    export OCTOFLOW_REAL_GATE=1 OCTOFLOW_GATE_CACHE="$WORKSPACE_TMP/cache"
                    OCTOFLOW_TEST_SEED=$(od -An -N8 -tu8 /dev/urandom | tr -d ' \\n')
                    if [ -z "$OCTOFLOW_TEST_SEED" ]; then
                        echo "no seed for the generated-input tests from /dev/urandom" >&2
                        exit 1
                    fi
                    export OCTOFLOW_TEST_SEED
                    echo "OCTOFLOW_TEST_SEED=$OCTOFLOW_TEST_SEED"
                    junit=target/nextest/ci/junit.xml
                    rm -f "$junit"
                    ci/cargo-lock.sh ci/rust-tests.sh
                    if [ ! -f "$junit" ]; then
                        echo "cargo-nextest wrote no $junit, so the Rust tests did not run" >&2
                        exit 1
                    fi
                '''
                // Whether the Rust is as `cargo fmt --all` formats it, as a JUnit report of one test for the Gate
                // stage, which lists a failure among the failing tests (ci/rustfmt.sh). It takes no lock: rustfmt
                // builds nothing.
                sh 'ci/rustfmt.sh'
            }
            post {
                always {
                    junit 'target/nextest/ci/junit.xml, target/rustfmt/junit.xml'
                }
            }
        }

        // The desktop app's frontend: its types, lint and Vitest tests; its Tauri shell's tests run in the Test
        // stage, with the rest of the Cargo workspace's. The gate judges no tests but those the Test stage reports, so
        // this stage stops the build instead, before Gate.
        stage('UI') {
            when { expression { return env.JUDGED_AT_PR != 'true' } }
            steps {
                dir('ui') {
                    sh '''
                        npm ci --no-audit --no-fund
                        npm run typecheck
                        npm run lint
                        npm test
                    '''
                }
                // The Gate stage's clippy checks the workspace's dependencies as cargo check does, not as the Test
                // stage's cargo-llvm-cov builds them, instrumented and in a target directory of its own: checked here,
                // under the lock, they leave clippy only the workspace's own crates to check.
                sh 'ci/cargo-lock.sh cargo check --locked --workspace --all-targets'
            }
        }

        // The quality gate (docs/quality-gate.md): pymarkdown, shellcheck, actionlint and clippy, the tests
        // and coverage of new code, judged on the lines the change adds. The build's own tree's ci/gate.pin,
        // pyproject.toml and gate.toml decide how the change is judged. The gate is the octoflow-gate release
        // ci/gate.pin names, fetched and checked against its sha256 by ci/fetch-gate.sh. A red gate fails the build, as
        // does a gate that could not judge, or could not be fetched, which writes no quality file.
        stage('Gate') {
            when { expression { return env.JUDGED_AT_PR != 'true' } }
            steps {
                script {
                    def base
                    if (env.CHANGE_TARGET) {
                        base = sh(returnStdout: true, script: 'ci/pr-base.sh').trim()
                    } else {
                        // A history rewritten since the last successful build, by a force-push, leaves that build's
                        // commit out of this checkout, where the gate cannot judge from it ("names no commit", exit
                        // status 2) in this and every later build; such a build measures from HEAD^1, as one with no
                        // successful build before it does.
                        def previous = env.PREVIOUS_SUCCESSFUL_COMMIT
                        def kept = previous && sh(returnStatus: true,
                                                  script: 'git cat-file -e "$PREVIOUS_SUCCESSFUL_COMMIT^{commit}"') == 0
                        base = kept ? previous : 'HEAD^1'
                    }
                    // The value reaches the shell as an environment variable, never as Groovy-built shell code.
                    def status
                    withEnv(["GATE_BASE=${base}"]) {
                        status = sh(returnStatus: true, script: '''
                            # Exit status 2 for whatever fails before the gate runs: a setup that failed is a gate
                            # that could not judge, never one that did not pass.
                            release="$WORKSPACE_TMP/gate"
                            rm -rf "$release" || exit 2
                            version=$(sh ci/fetch-gate.sh ci/gate.pin "$release") || exit 2
                            echo "Gate: octoflow-gate $version, its tarball checked against ci/gate.pin's sha256"
                            # The Test stage ran the tests; their reports replace gate.toml's [[test]] commands.
                            # uv run puts pymarkdown, which the gate runs, on PATH.
                            uv run --frozen "$release/gate" --base "$GATE_BASE" \\
                                --config gate.toml \\
                                --tests target/nextest/ci/junit.xml --tests target/rustfmt/junit.xml \\
                                --coverage coverage-rust.xml --out quality.md
                        ''')
                    }
                    if (status == 1) {
                        error('The quality gate did not pass; see quality.md')
                    } else if (status != 0) {
                        error("The quality gate could not judge the change (exit status ${status})")
                    }
                }
            }
            post {
                always {
                    // Neither file when the gate could not judge.
                    archiveArtifacts artifacts: 'quality.md, quality.json', allowEmptyArchive: true
                }
            }
        }

        // A pull request into main must raise the release version over main's, unless it changes only docs, tests, CI
        // or repository tooling: the Release stage releases a merge into main only when its version has no release
        // yet, so a merge that kept it would go unreleased (ci/version-raised.sh, which lists those paths). Nor may
        // it while an older open pull request into main changes what is released too. After Gate, so that a build
        // that fails it has reported its tests and gate first.
        stage('Version') {
            when { changeRequest target: 'main' }
            steps {
                // The open pull requests are listed here, before the script fetches main, so that one missing from
                // the list has merged into what it fetches. The token's step takes nothing from the workspace: its
                // PATH, worked out here in Groovy rather than by a shell found on the build's PATH, is the build's
                // without relative directories or any under the workspace, so its shell, gh and every other command
                // are looked up past .tools/bin; gh runs from / and is told the repository by CHANGE_URL, so the
                // checkout's git config is not read; and ci/version-raised.sh, which the pull request can change,
                // runs after, without the token. Not guaranteed: a directory outside the workspace that the build
                // user can write to passes the filter, so a tool a pull request's code left there runs with the
                // token, which the user accepted (2026-11-24, todo-tickets: only this repository's pull requests
                // build on the agent); and a pull request whose own Jenkinsfile Jenkins runs controls all of this.
                script {
                    def trusted = env.PATH.split(':').findAll { it.startsWith('/') && !it.startsWith(env.WORKSPACE) }
                    withEnv(["PATH=${trusted.join(':')}"]) {
                        withCredentials([string(credentialsId: 'GitHub-Agents', variable: 'GH_TOKEN')]) {
                            sh '''
                                list="$WORKSPACE/open-pull-requests.txt"
                                cd / && gh pr list --repo "${CHANGE_URL%/pull/*}" --base "$CHANGE_TARGET" \\
                                    --state open --limit 1000 --json number --jq '.[].number' > "$list"
                            '''
                        }
                    }
                }
                sh 'OPEN_PULL_REQUESTS="$WORKSPACE/open-pull-requests.txt" ci/version-raised.sh'
            }
        }

        // The Linux binary and the desktop app around it, built on every pull request and main build, so a change
        // that breaks either fails before it is merged; Release attaches them. The macOS ones come from .github/workflows/macos-binary.yml, with the same scripts, since Jenkins has no
        // macOS machine.
        stage('Package') {
            steps {
                sh 'packaging/build-binary.sh'
                sh 'ci/cargo-lock.sh packaging/build-desktop.sh'
            }
        }

        // A merge into main that raises the version (ci/release-version.sh) releases it; any other merge finds that
        // release already there. gh creates the tag v<version> on GitHub at the commit built, so Jenkins needs
        // no git push credentials.
        stage('Release') {
            when { branch 'main' }
            steps {
                withCredentials([string(credentialsId: 'GitHub-Agents', variable: 'GH_TOKEN')]) {
                    sh '''
                        version=$(ci/release-version.sh)
                        tag="v$version"
                        sha=$(git rev-parse HEAD)
                        if gh release view "$tag" >/dev/null 2>gh-release-view.err; then
                            echo "$tag is already released"
                            exit 0
                        fi
                        # Anything but a missing release, e.g. a bad token, must fail rather than release twice.
                        if ! grep -q 'release not found' gh-release-view.err; then
                            cat gh-release-view.err >&2
                            exit 1
                        fi
                        # Package builds them on every build that reaches Release.
                        for f in octoflow-linux-x86_64 octoflow-desktop-linux-x86_64.deb \
                                 octoflow-desktop-linux-x86_64.AppImage; do
                            [ -f "dist/$f.sha256" ] || { echo "no dist/$f; run the whole build" >&2; exit 1; }
                        done
                        # The crates' licenses, one file for every asset, the macOS ones too (packaging/about.toml), and
                        # the desktop app's npm packages' (packaging/build-desktop.sh), the same on every platform.
                        for f in octoflow-third-party-licenses.html octoflow-desktop-npm-licenses.html; do
                            [ -f "dist/$f" ] || { echo "no dist/$f; run the whole build" >&2; exit 1; }
                        done
                        # A release's own words, docs/releases/v<version>.md, when there is such a file: GitHub puts
                        # them before the notes it generates from the pull requests.
                        notes=docs/releases/$tag.md
                        set --
                        [ ! -f "$notes" ] || set -- --notes-file "$notes"
                        # Publishing it starts .github/workflows/macos-binary.yml, which adds the macOS binary and app.
                        gh release create "$tag" "$@" \
                            dist/octoflow-linux-x86_64 dist/octoflow-linux-x86_64.sha256 \
                            dist/octoflow-desktop-linux-x86_64.deb dist/octoflow-desktop-linux-x86_64.deb.sha256 \
                            dist/octoflow-desktop-linux-x86_64.AppImage \
                            dist/octoflow-desktop-linux-x86_64.AppImage.sha256 \
                            dist/octoflow-third-party-licenses.html dist/octoflow-desktop-npm-licenses.html \
                            --target "$sha" --title "$tag" --generate-notes
                    '''
                }
            }
        }
    }

    // Mail on a failure and on the first green build after one, never on a run that merely succeeded again.
    post {
        always {
            cleanWs()
        }
        failure {
            emailext(
                to: 'david.siegl95@gmail.com',
                subject: "FAILED  ${env.JOB_NAME} #${env.BUILD_NUMBER}",
                body: """Failed - see the attached log.

Gate report, if the build reached the Gate stage: ${env.BUILD_URL}artifact/quality.md
${env.BUILD_URL}console""",
                attachLog: true,
                compressLog: true
            )
        }
        fixed {
            emailext(
                to: 'david.siegl95@gmail.com',
                subject: "RECOVERED  ${env.JOB_NAME} #${env.BUILD_NUMBER}",
                body: "${env.BUILD_URL}"
            )
        }
    }
}
