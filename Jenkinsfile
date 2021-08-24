#!/usr/bin/env groovy

// Searches previous builds to find first non aborted one
def getLastNonAbortedBuild(build) {
  if (build == null) {
    return null;
  }

  if(build.result.toString().equals("ABORTED")) {
    return getLastNonAbortedBuild(build.getPreviousBuild());
  } else {
    return build;
  }
}

// Send out a slack message if branch got broken or has recovered
def notifySlackUponStateChange(build) {
  def cur = build.getResult()
  def prev = getLastNonAbortedBuild(build.getPreviousBuild())?.getResult()
  if (cur != prev) {
    if (cur == 'SUCCESS') {
      slackSend(
        channel: '#mayastor-control-plane',
        color: 'normal',
        message: "Branch ${env.BRANCH_NAME} has been fixed :beers: (<${env.BUILD_URL}|Open>)"
      )
    } else if (prev == 'SUCCESS') {
      slackSend(
        channel: '#mayastor-control-plane',
        color: 'danger',
        message: "Branch ${env.BRANCH_NAME} is broken :face_with_raised_eyebrow: (<${env.BUILD_URL}|Open>)"
      )
    }
  }
}

// Will ABORT current job for cases when we don't want to build
if (currentBuild.getBuildCauses('jenkins.branch.BranchIndexingCause') &&
    BRANCH_NAME == "develop") {
    print "INFO: Branch Indexing, aborting job."
    currentBuild.result = 'ABORTED'
    return
}

// Only schedule regular builds on develop branch, so we don't need to guard against it
String cron_schedule = BRANCH_NAME == "develop" ? "0 2 * * *" : ""

run_linter = true
rust_test = true
bdd_test = true

pipeline {
  agent none
  options {
    timeout(time: 1, unit: 'HOURS')
  }
  parameters {
    booleanParam(defaultValue: true, name: 'build_images')
  }
  triggers {
    cron(cron_schedule)
  }

  stages {
    stage('init') {
      agent { label 'nixos-mayastor' }
      steps {
        step([
          $class: 'GitHubSetCommitStatusBuilder',
          contextSource: [
            $class: 'ManuallyEnteredCommitContextSource',
            context: 'continuous-integration/jenkins/branch'
          ],
          statusMessage: [ content: 'Pipeline started' ]
        ])
      }
    }
    stage('linter') {
      agent { label 'nixos-mayastor' }
      when {
        beforeAgent true
        not {
          anyOf {
            branch 'master'
            branch 'release/*'
            expression { run_linter == false }
          }
        }
      }
      steps {
        sh 'printenv'
        sh 'nix-shell --run "cargo fmt --all -- --check"'
        sh 'nix-shell --run "cargo clippy --all-targets -- -D warnings"'
        sh 'nix-shell --run "./scripts/generate-openapi-bindings.sh"'
        sh 'nix-shell --run "black tests/bdd"'
      }
    }
    stage('test') {
      when {
        beforeAgent true
        not {
          anyOf {
            branch 'master'
            branch 'release/*'
          }
        }
      }
      parallel {
        stage('rust unit tests') {
          when{
            expression { rust_test == true }
          }
          agent { label 'nixos-mayastor' }
          steps {
            sh 'printenv'
            sh 'nix-shell --run "./scripts/ctrlp-cargo-test.sh"'
          }
          post {
            always {
              // in case of abnormal termination of any nvmf test
              sh 'sudo nvme disconnect-all'
            }
          }
        }
        stage('BDD tests') {
          when{
            expression { bdd_test == true }
          }
          agent { label 'nixos-mayastor' }
          steps {
            sh 'printenv'
            sh 'nix-shell --run "cargo build --bins"'
            sh 'nix-shell --run "./scripts/bdd-tests.sh"'
          }
        }
      }// parallel stages block
    }// end of test stage
    stage('build images') {
      agent { label 'nixos-mayastor' }
      when {
        beforeAgent true
        allOf {
          expression { params.build_images == true }
          anyOf {
            branch 'master'
            branch 'release/*'
            branch 'develop'
            branch 'trying'
          }
        }
      }
      steps {
        sh './scripts/release.sh --skip-publish'
      }
      post {
        always {
          sh 'docker image prune --all --force'
        }
      }
    }
  }

  // The main motivation for post block is that if all stages were skipped
  // (which happens when running cron job and branch != develop) then we don't
  // want to set commit status in github (jenkins will implicitly set it to
  // success).
  post {
    always {
      node(null) {
        script {
          // If no tests were run then we should neither be updating commit
          // status in github nor send any slack messages
          if (currentBuild.result != null) {
            step([
                    $class            : 'GitHubCommitStatusSetter',
                    errorHandlers     : [[$class: "ChangingBuildStatusErrorHandler", result: "UNSTABLE"]],
                    contextSource     : [
                            $class : 'ManuallyEnteredCommitContextSource',
                            context: 'continuous-integration/jenkins/branch'
                    ],
                    statusResultSource: [
                            $class : 'ConditionalStatusResultSource',
                            results: [
                                    [$class: 'AnyBuildResult', message: 'Pipeline result', state: currentBuild.getResult()]
                            ]
                    ]
            ])
            if (env.BRANCH_NAME == 'develop') {
              notifySlackUponStateChange(currentBuild)
            }
          }
        }
      }
    }
  }
}
