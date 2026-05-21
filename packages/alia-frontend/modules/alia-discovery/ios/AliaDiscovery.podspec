Pod::Spec.new do |s|
  s.name           = 'AliaDiscovery'
  s.version        = '0.1.0'
  s.summary        = 'Local mDNS and BLE discovery for Alia.'
  s.description    = 'Local Expo module for discovering Alia Server and provisioning Alia Embedded over BLE.'
  s.license        = 'MIT'
  s.author         = 'Alia'
  s.homepage       = 'https://github.com/zerlight/project-alia'
  s.platforms      = {
    :ios => '15.1'
  }
  s.swift_version  = '5.9'
  s.source         = { git: 'https://github.com/zerlight/project-alia.git' }
  s.static_framework = true

  s.dependency 'ExpoModulesCore'

  s.source_files = "**/*.{h,m,swift}"
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    'SWIFT_COMPILATION_MODE' => 'wholemodule'
  }
end
