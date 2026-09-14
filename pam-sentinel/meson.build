project('pam-sentinel', 'c',
  version: '0.1.0',
  default_options: ['c_std=c99']
)

pam_dep  = dependency('pam')
dbus_dep = dependency('dbus-1')

shared_library('pam_sentinel',
  'pam_sentinel.c',
  dependencies: [pam_dep, dbus_dep],
  name_prefix: '',
  install: true,
  install_dir: '/usr/lib/security'
)
