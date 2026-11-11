pipeline {
    // Agents wrote the code this builds, so it runs where they already do: the agent on their own box.
    // Never on the agents that deploy the homelab. Jenkins enforces that per node, not this label.
    agent { label 'untrusted' }

    // Empty in an ordinary build, which builds its own branch or pull request.
    parameters {
        string(name: 'GIT_REF', defaultValue: '', description: 'Branch to build, e.g. orchestrator-ci/cd3492-1-q2')
    }

    options {
        skipDefaultCheckout()
        // Per job, and per branch or pull request in the multibranch one; a discarded build takes its archived
        // quality.md with it, and a resumed run that still names it builds its ref again. Jenkins never discards the
        // last successful build, which GIT_PREVIOUS_SUCCESSFUL_COMMIT names for the Gate stage.
        buildDiscarder(logRotator(numToKeepStr: '30'))
    }

    environment {
        UV_NO_PROGRESS = '1'
        // For the Test, UI, Gate and Package stages: crates are fetched into the workspace, which cleanWs empties, so
        // no build compiles sources an earlier build left behind.
        CARGO_HOME     = "${env.WORKSPACE}/.cargo-home"
        // Tools the Install stage downloads into the workspace (ci/install-tools.sh), first on PATH for every stage.
        PATH           = "${env.WORKSPACE}/.tools/bin:${env.PATH}"
        // The dev profile, which the Test stage's test build and clippy's checks inherit, without debug info. Debug
        // info for the GTK and Tauri crates took enough memory that rustc was OOM-killed with two builds on
        // agentbox0. ci/cargo-lock.sh now keeps two builds' cargo steps apart, except a quality build's, which never
        // waits for the lock, and the Gate stage's clippy, which the gate runs outside it; building without debug info
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
        // A quality build (GIT_REF set) records what it judges as its build description, before Install runs any
        // code of the change: exactly `judged tree=<sha> parent=<sha> main=<sha>`, full lowercase shas, one space
        // apart. tree is the snapshot's tree, parent its only parent, the run's base, and main the origin/main
        // commit whose gate and rules the Gate stage judges with. `orchestrator judged-in-quality` parses it, by
        // JUDGED_DESCRIPTION. A build restarted from a later stage records none, so nothing trusts it. No later stage
        // may fetch GIT_REF: the orchestrator deletes that branch once the Gate stage has archived quality.md, while
        // the build may still run (docs/quality-gate.md, "The calls").
        stage('Checkout') {
            steps {
                script {
                    def checkedOut
                    if (params.GIT_REF) {
                        checkedOut = checkout([$class: 'GitSCM',
                                               branches: [[name: "refs/heads/${params.GIT_REF}"]],
                                               userRemoteConfigs: scm.userRemoteConfigs])
                        sh 'git fetch --quiet origin +refs/heads/main:refs/remotes/origin/main'
                        env.JUDGED_MAIN = sh(returnStdout: true,
                                             script: 'git rev-parse --verify "origin/main^{commit}"').trim()
                        // Each assignment fails the step on its own, under sh's -e, where a substitution inside
                        // printf's arguments would not.
                        currentBuild.description = sh(returnStdout: true, script: '''
                            tree=$(git rev-parse --verify 'HEAD^{tree}')
                            parent=$(git rev-parse --verify 'HEAD^')
                            printf 'judged tree=%s parent=%s main=%s' "$tree" "$parent" "$JUDGED_MAIN"
                        ''').trim()
                    } else {
                        checkedOut = checkout scm
                    }
                    // The Gate stage measures a build of main from here.
                    env.PREVIOUS_SUCCESSFUL_COMMIT = checkedOut.GIT_PREVIOUS_SUCCESSFUL_COMMIT ?: ''
                }
            }
        }

        // A build of main skips Test and Gate when a pull request build already judged its tree
        // (ci/judged-at-pr.sh). Whatever stops the script from saying so, the build judges the tree itself.
        stage('Judged at PR') {
            when {
                branch 'main'
                expression { return !params.GIT_REF }
            }
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
                // of its own.
                sh 'uv sync --frozen'
                // The Test stage's TestClippy and the Gate stage's gate run the real clippy, and the Test stage's
                // cargo-llvm-cov reads its coverage data with llvm-tools' llvm-profdata and llvm-cov, which must be the
                // toolchain's own LLVM. Both are rustup components the agent image provides: its rustup home,
                // /usr/local/rustup, is root's, so a build cannot add to it (PR-163's builds 1-3 failed adding
                // llvm-tools-preview: "component download failed ... No such file or directory"). This step names a
                // missing one here rather than letting a later stage fail on it. rustup reads rust-toolchain.toml here,
                // so it lists the components of the toolchain that file pins, which must be the image's too: rustup
                // cannot install another one either.
                sh '''
                    installed=$(rustup component list --installed)
                    for component in clippy llvm-tools; do
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
            environment {
                // Temporary files on the agent's mounted volume, beside the workspace, rather than in /tmp, for this
                // stage and the Gate stage. The tests and the gate build Rust in temporary directories and run what
                // they build: the test fixtures' build scripts and test binaries, and cargo-mutants on the gate's copy
                // of a tree. /tmp is in the container's writable layer, where the agent host's Falco reports every
                // program run from a file written there ("Drop and execute new binary in container", Critical): 109
                // mails in 15 minutes on 2026-11-03. The workspace's own target/ is on the volume and was never
                // reported. Python's tempfile takes /tmp, silently, for a TMPDIR that does not exist, so each stage
                // creates it first. The Package stage still uses /tmp (TODO.md).
                TMPDIR = "${env.WORKSPACE_TMP}/tmp"
            }
            steps {
                // What an earlier build of this workspace left there goes first: on the volume, unlike in the
                // container's /tmp, it would otherwise stay. The chmod is for a directory a killed test left without write permission, such as
                // engine/src/host/tests.rs's 0o500 one; a clear-out that still fails only warns, since failing here
                // would fail every later build of this workspace until someone cleaned the volume by hand.
                sh '''
                    if [ -e "$TMPDIR" ]; then
                        { chmod -R u+rwX "$TMPDIR" && rm -rf "$TMPDIR"; } ||
                            echo "could not empty $TMPDIR; what an earlier build left there stays" >&2
                    fi
                    mkdir -p "$TMPDIR"
                '''
                // A failing test does not stop the build here: the Gate stage judges it with everything else, so a
                // quality build's report lists failing tests and findings together.
                //
                // The Rust tests of every member of the top-level Cargo workspace (Cargo.toml), the engine's, its e2e
                // scenarios' and docs check's, the desktop shell's and xtask's (which build the eval tasks' toy crates,
                // and the engine into target/xtask-engine), with line coverage, for the Gate stage likewise
                // (ci/rust-tests.sh, which says what it writes and when it fails). Once the tests are built,
                // --ignore-run-fail turns any failure of nextest's run into a warning (run_nextest in cargo-llvm-cov
                // 0.9.1's src/main.rs), also one that runs no test, such as a test binary that cannot list its tests;
                // so the JUnit report is removed first and must be there after. The lock as in the UI stage, which
                // says why a quality build may skip it.
                //
                // A quality build (GIT_REF set) leaves the e2e scenarios out, by a nextest filter on their test binary,
                // e2e, so that the docs check beside them still runs, when its change, against HEAD^, the run's base,
                // touches nothing they depend on: the engine and the e2e package under engine/, the Cargo workspace it
                // builds in, the example workflows they run, ci/, which pins the gitleaks they run, and the desktop app's
                // recording, which the e2e binary's app_event_stream compares with what serve sends. Their Python
                // forerunner took quality build #59 past the orchestrator's QUALITY_TIMEOUT (1299 s of 1200). A git
                // diff that fails counts as a change, so they run. Pull request builds always run them.
                //
                // OCTOFLOW_TEST_SEED is the run seed of the engine's generated-input tests (engine/src/test_support.rs
                // says what it changes), a fresh one each build. It is echoed here, whatever the tests do, as the
                // assignment that runs a build's cases again.
                sh '''
                    OCTOFLOW_TEST_SEED=$(od -An -N8 -tu8 /dev/urandom | tr -d ' \\n')
                    if [ -z "$OCTOFLOW_TEST_SEED" ]; then
                        echo "no seed for the generated-input tests from /dev/urandom" >&2
                        exit 1
                    fi
                    export OCTOFLOW_TEST_SEED
                    echo "OCTOFLOW_TEST_SEED=$OCTOFLOW_TEST_SEED"
                    junit=target/nextest/ci/junit.xml
                    rm -f "$junit"
                    set --
                    if [ -n "${GIT_REF:-}" ] && git diff --quiet HEAD^ HEAD -- engine Cargo.toml Cargo.lock \
                            rust-toolchain.toml examples ci ui/src/test/recorded-stream.json; then
                        set -- -E 'not (package(octoflow-e2e) and binary(e2e))'
                    fi
                    ci/cargo-lock.sh ${GIT_REF:+--no-wait} ci/rust-tests.sh "$@"
                    if [ ! -f "$junit" ]; then
                        echo "cargo-nextest wrote no $junit, so the Rust tests did not run" >&2
                        exit 1
                    fi
                '''
            }
            post {
                always {
                    junit 'target/nextest/ci/junit.xml'
                }
            }
        }

        // The desktop app's frontend: its types, lint and Vitest tests; its Tauri shell's tests run in the Test
        // stage, with the rest of the Cargo workspace's. The gate judges no tests but those the Test stage reports, so
        // this stage stops the build instead, before Gate: a quality round then reports BUILD_FAILED with the
        // console's end, where a failure after Gate would hide under quality.md's passing GATE line.
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
                // under the lock, they leave clippy only the workspace's own crates to check. A quality build (GIT_REF
                // set) runs within the orchestrator's QUALITY_TIMEOUT, which a wait behind another build's Package
                // stage would exceed, so it takes the lock only when it is free.
                sh 'ci/cargo-lock.sh ${GIT_REF:+--no-wait} cargo check --locked --workspace --all-targets'
            }
        }

        // The quality gate (docs/quality-gate.md): pymarkdown, shellcheck, actionlint and clippy, the tests
        // and coverage of new code, judged on the lines the change adds. The judge is the tree whose ci/gate.pin,
        // pyproject.toml and gate.toml decide how the change is judged: a quality build's is the main commit Checkout
        // named, so a change cannot loosen how it is itself judged, but for clippy's rules, which are the tree's own
        // Cargo.toml (TODO.md); any other build's is its own tree. The gate is the octoflow-gate release the judge's
        // ci/gate.pin names, fetched and checked against its sha256 by the judge's ci/fetch-gate.sh. A quality build
        // goes UNSTABLE on a red gate, and a pull request or main fails. A gate that could not judge, or could not be
        // fetched, fails any build and writes no quality file.
        stage('Gate') {
            when { expression { return env.JUDGED_AT_PR != 'true' } }
            environment {
                // Off the writable layer, as the Test stage's says why.
                TMPDIR = "${env.WORKSPACE_TMP}/tmp"
            }
            steps {
                sh 'mkdir -p "$TMPDIR"'
                script {
                    def base
                    if (params.GIT_REF) {
                        // The snapshot's only parent is the run's base.
                        base = 'HEAD^'
                    } else if (env.CHANGE_TARGET) {
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
                            # The judge's files, a quality build's copied out of the main commit Checkout named;
                            # pyproject.toml beside gate.toml, where --config makes the gate read it. Exit status 2 for
                            # whatever fails before the gate runs: a setup that failed is a gate that could not judge,
                            # never one that did not pass.
                            judge=.
                            if [ -n "${GIT_REF:-}" ]; then
                                : "${JUDGED_MAIN:?is unset; run the whole build, not a restart from a later stage}"
                                judge="$WORKSPACE_TMP/judge"
                                rm -rf "$judge" && mkdir -p "$judge/ci" || exit 2
                                for f in pyproject.toml gate.toml ci/gate.pin ci/fetch-gate.sh; do
                                    git show "$JUDGED_MAIN:$f" > "$judge/$f" || exit 2
                                done
                            fi
                            release="$WORKSPACE_TMP/gate"
                            rm -rf "$release" || exit 2
                            version=$(sh "$judge/ci/fetch-gate.sh" "$judge/ci/gate.pin" "$release") || exit 2
                            echo "Gate: octoflow-gate $version, its tarball checked against ci/gate.pin's sha256"
                            # The Test stage ran the tests; their reports replace gate.toml's [[test]] commands.
                            # uv run puts pymarkdown, which the gate runs, on PATH.
                            uv run --frozen "$release/gate" --base "$GATE_BASE" \\
                                --config "$judge/gate.toml" \\
                                --tests target/nextest/ci/junit.xml \\
                                --coverage coverage-rust.xml --out quality.md
                        ''')
                    }
                    if (status == 1 && params.GIT_REF) {
                        unstable('The quality gate did not pass; see quality.md')
                    } else if (status == 1) {
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
        // yet, so a merge that kept it would go unreleased (ci/version-raised.sh, which lists those paths). After
        // Gate, so that a build that fails it has reported its tests and gate first.
        stage('Version') {
            when { changeRequest target: 'main' }
            steps {
                sh 'ci/version-raised.sh'
            }
        }

        // The Linux binary and the desktop app around it, built on every pull request and main build, so a change
        // that breaks either fails before it is merged; Release attaches them. Not in a quality build (GIT_REF set):
        // nothing uses a snapshot's package, since the change is packaged by its pull request's build before it
        // merges, and the orchestrator run that triggered the build has taken the Gate stage's quality.md by then.
        // The macOS ones come from .github/workflows/macos-binary.yml, with the same scripts, since Jenkins has no
        // macOS machine.
        stage('Package') {
            when { expression { return !params.GIT_REF } }
            steps {
                sh 'packaging/build-binary.sh'
                sh 'ci/cargo-lock.sh packaging/build-desktop.sh'
            }
        }

        // A merge into main that raises the version (ci/release-version.sh) releases it; any other merge finds that
        // release already there. gh creates the tag v<version> on GitHub at the commit built, so Jenkins needs
        // no git push credentials.
        stage('Release') {
            when {
                branch 'main'
                // A build pointed at main's job with GIT_REF checks out that branch, never to be released.
                expression { return !params.GIT_REF }
            }
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

    // Mail on a failure and on the first green build after one, never on a run that merely succeeded again. Not for
    // a quality build: the orchestrator run that triggered it hands its result to the Builder.
    post {
        always {
            cleanWs()
        }
        failure {
            script {
                if (!params.GIT_REF) {
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
            }
        }
        fixed {
            script {
                if (!params.GIT_REF) {
                    emailext(
                        to: 'david.siegl95@gmail.com',
                        subject: "RECOVERED  ${env.JOB_NAME} #${env.BUILD_NUMBER}",
                        body: "${env.BUILD_URL}"
                    )
                }
            }
        }
    }
}
