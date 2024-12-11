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

def mainBranches() {
    return BRANCH_NAME == "develop" || BRANCH_NAME.startsWith("release/");
}

def cronSchedule() {
    node {
        if ( env.CRON_SCHEDULE_URL == null ) {
            println "ERROR: No cron schedule url in the environment"
            return ""
        }
        println "DEBUG: Fetching Cron Schedule from ${env.CRON_SCHEDULE_URL}"
        def YAML = sh (script: "curl ${env.CRON_SCHEDULE_URL}", returnStdout: true).trim()
        def schedules = readYaml text: YAML
        def branch_cfg = schedules[BRANCH_NAME] ? schedules[BRANCH_NAME] : schedules["default"]
        if ( branch_cfg == null ) {
            println "ERROR: Failed to retrieve the cron schedule for the branch: ${BRANCH_NAME}"
            return ""
        }

        def job_name_split = env.JOB_NAME.tokenize('/') as String[];
        def project_name = job_name_split[0]
        if ( project_name == null ) {
            println "ERROR: No project name for: ${env.JOB_NAME}"
            return ""
        }

        def project_cfg = branch_cfg[project_name] ? branch_cfg[project_name] : branch_cfg["default"]
        if ( project_cfg == null ) {
            println "ERROR: No cron schedule for: ${project_name}"
            return ""
        }

        println "INFO: Cron Schedule for ${project_name}/${BRANCH_NAME}: ${project_cfg}"
        return project_cfg
    }
}
def dockerId() {
  script {
    dockerId = sh (script: "./utils/dependencies/scripts/git-org-name.sh --case upper", returnStdout: true).trim() + "_DOCKERHUB"
    println "Using docker id: ${dockerId}"
    return dockerId
  }
}

// TODO: Use multiple choices
run_linter = true
rust_test = true
bdd_test = true
run_tests = params.run_tests
build_images = params.build_images

// Will skip steps for cases when we don't want to build
if (currentBuild.getBuildCauses('jenkins.branch.BranchIndexingCause') && mainBranches()) {
    print "INFO: Branch Indexing, skip tests and push the new images."
    run_tests = false
    build_images = true
}

pipeline {
  agent none
  options {
    timeout(time: 2, unit: 'HOURS')
  }
  parameters {
    booleanParam(defaultValue: false, name: 'build_images')
    booleanParam(defaultValue: true, name: 'run_tests')
  }
  triggers {
    cron(cronSchedule())
  }
  environment {
    CARGO_INCREMENTAL = "0"
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
            branch 'release/*'
            expression { run_linter == false }
          }
        }
      }
      steps {
        sh 'printenv'
        sh 'nix-shell --run "./scripts/rust/generate-openapi-bindings.sh"'
        sh 'nix-shell --run "./scripts/rust/linter.sh"'
        sh 'nix-shell --run "black --check tests/bdd"'
        sh 'nix-shell --run "./scripts/git/check-submodule-branches.sh"'
      }
    }
    stage('test') {
      when {
        beforeAgent true
        expression { run_tests == true }
      }
      parallel {
        stage('rust unit tests') {
          when{
            expression { rust_test == true }
          }
          agent { label 'nixos-mayastor' }
          steps {
            withCredentials([usernamePassword(credentialsId: dockerId(), usernameVariable: 'USERNAME', passwordVariable: 'PASSWORD')]) {
              sh 'echo $PASSWORD | docker login -u $USERNAME --password-stdin'
            }
            sh 'printenv'
            sh 'nix-shell --run "cargo build --bins"'
            sh 'nix-shell --run "deployer start --image-pull-policy always -w 60s && deployer stop"'
            // builds the tests
            sh 'nix-shell --run "./scripts/rust/test.sh --no-run"'
            sh 'nix-shell --run "./scripts/rust/test.sh"'
          }
          post {
            always {
              sh 'nix-shell --run "./scripts/rust/deployer-cleanup.sh"'
            }
          }
        }
        stage('BDD tests') {
          when{
            expression { bdd_test == true }
          }
          agent { label 'nixos-mayastor' }
          steps {
            withCredentials([usernamePassword(credentialsId: dockerId(), usernameVariable: 'USERNAME', passwordVariable: 'PASSWORD')]) {
              sh 'echo $PASSWORD | docker login -u $USERNAME --password-stdin'
            }
            sh 'printenv'
            sh 'nix-shell --run "cargo build --bins"'
            sh 'nix-shell --run "deployer start --image-pull-policy always -w 60s && deployer stop"'
            sh 'nix-shell --run "./scripts/python/test.sh"'
          }
          post {
            always {
              junit 'report.xml'
              sh 'nix-shell --run "./scripts/python/test-residue-cleanup.sh"'
            }
          }
        }
        stage('image build test') {
          when {
            branch 'staging'
          }
          agent { label 'nixos-mayastor' }
          steps {
            sh 'printenv'
            sh './scripts/nix/git-submodule-init.sh --force'
            sh './scripts/release.sh --skip-publish --debug --build-bins'
          }
        }
      }// parallel stages block
    }// end of test stage
    stage('build and push images') {
      agent { label 'nixos-mayastor' }
      when {
        beforeAgent true
        anyOf {
          expression { build_images == true }
          anyOf {
            branch 'release/*'
            branch 'develop'
          }
        }
      }
      steps {
        withCredentials([usernamePassword(credentialsId: dockerId(), usernameVariable: 'USERNAME', passwordVariable: 'PASSWORD')]) {
            sh 'echo $PASSWORD | docker login -u $USERNAME --password-stdin'
        }
        sh 'printenv'
        sh './scripts/nix/git-submodule-init.sh --force'
        sh './scripts/release.sh'
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
          }
        }
      }
    }
  }
}
