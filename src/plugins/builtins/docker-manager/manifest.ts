import type { PluginManifest } from '@plugin-api/shared'
import {
  DOCKER_CONFIRM_DESTRUCTIVE_DEFAULT,
  DOCKER_DEFAULT_LOG_TAIL_LINES,
  DOCKER_DEFAULT_REFRESH_INTERVAL_MS,
  DOCKER_DEFAULT_SHOW_STOPPED,
  DOCKER_DEFAULT_SOCKET_PATH,
  DOCKER_DEFAULT_STATS_INTERVAL_MS
} from './defaults'
import { PLUGIN_ID_DOCKER_MANAGER, DOCKER_PANEL_VIEW_ID } from './id'

export const dockerManagerManifest: PluginManifest = {
  id: PLUGIN_ID_DOCKER_MANAGER,
  name: 'Docker',
  version: '1.0.0',
  description:
    'Manage Docker on the session host: containers (start, stop, restart, pause, remove, logs, live stats, shell), images, volumes, and networks. Talks to the Docker Engine API over the daemon socket on SSH and Local sessions.',
  activation: 'manual',
  source: 'builtin',
  contributes: {
    toolbar: { label: 'Docker' },
    settingsHeading: 'Docker',
    settingsSchema: [
      {
        key: 'confirmDestructive',
        label: 'Confirm destructive actions',
        type: 'boolean',
        default: DOCKER_CONFIRM_DESTRUCTIVE_DEFAULT,
        description:
          'Ask before removing a container, killing it, deleting an image, volume, or network, or pruning.'
      }
    ],
    hostSettingsHeading: 'Docker',
    hostSettingsSchema: [
      {
        key: 'socketPath',
        label: 'Daemon socket path',
        type: 'string',
        default: DOCKER_DEFAULT_SOCKET_PATH,
        description:
          'Path to the Docker daemon socket on this host. Rootless Docker and Docker Desktop use different paths (for example /run/user/1000/docker.sock).'
      },
      {
        key: 'showStopped',
        label: 'Show stopped containers',
        type: 'boolean',
        default: DOCKER_DEFAULT_SHOW_STOPPED,
        description: 'Include exited and created containers in the list.'
      },
      {
        key: 'refreshIntervalMs',
        label: 'List refresh interval (ms)',
        type: 'number',
        default: DOCKER_DEFAULT_REFRESH_INTERVAL_MS,
        description: 'How often the container list refreshes while the panel is visible.'
      },
      {
        key: 'statsIntervalMs',
        label: 'Stats interval (ms)',
        type: 'number',
        default: DOCKER_DEFAULT_STATS_INTERVAL_MS,
        description: 'How often live container stats are sampled while the stats tab is open.'
      },
      {
        key: 'logTailLines',
        label: 'Log tail lines',
        type: 'number',
        default: DOCKER_DEFAULT_LOG_TAIL_LINES,
        description: 'How many historical log lines to load when opening container logs.'
      },
      {
        key: 'sudoPassword',
        label: 'sudo password',
        type: 'string',
        default: '',
        secret: true,
        description:
          'Password piped to `sudo -S` when installing the Docker engine on this host. Leave empty to use passwordless sudo. Stored unencrypted in hosts.json.'
      }
    ],
    views: [
      {
        id: DOCKER_PANEL_VIEW_ID,
        placement: 'split-right',
        title: 'Docker'
      }
    ],
    api: {
      methods: [
        {
          name: 'list_containers',
          label: 'List containers',
          description:
            'List Docker containers on this session host. Set all=true to include stopped containers.',
          parameters: {
            type: 'object',
            properties: { all: { type: 'boolean' } }
          }
        },
        {
          name: 'inspect_container',
          description: 'Return detailed information about one container by id or name.',
          parameters: {
            type: 'object',
            properties: { id: { type: 'string' } },
            required: ['id']
          }
        },
        {
          name: 'get_container_logs',
          label: 'Get container logs',
          description: 'Return the most recent log lines for a container.',
          parameters: {
            type: 'object',
            properties: {
              id: { type: 'string' },
              tail: { type: 'number' },
              timestamps: { type: 'boolean' }
            },
            required: ['id']
          }
        },
        {
          name: 'get_container_stats',
          label: 'Get container stats',
          description: 'Return a one-shot CPU, memory, network, and disk sample for a container.',
          parameters: {
            type: 'object',
            properties: { id: { type: 'string' } },
            required: ['id']
          }
        },
        {
          name: 'list_images',
          description: 'List Docker images on this session host.',
          parameters: { type: 'object', properties: {} }
        },
        {
          name: 'list_volumes',
          description: 'List Docker volumes on this session host.',
          parameters: { type: 'object', properties: {} }
        },
        {
          name: 'list_networks',
          description: 'List Docker networks on this session host.',
          parameters: { type: 'object', properties: {} }
        },
        {
          name: 'container_action',
          label: 'Container action',
          description:
            'Start, stop, restart, pause, unpause, kill, or remove a container. Destructive actions (kill, remove) cannot be undone.',
          parameters: {
            type: 'object',
            properties: {
              id: { type: 'string' },
              action: {
                type: 'string',
                enum: ['start', 'stop', 'restart', 'pause', 'unpause', 'kill', 'remove']
              }
            },
            required: ['id', 'action']
          },
          defaultPermission: 'ask'
        },
        {
          name: 'update_container',
          label: 'Update container',
          description:
            'Pull the container\'s image tag and recreate the container in place from its current configuration (ports, volumes, environment, restart policy, networks). The old container is restored if the replacement fails to start.',
          parameters: {
            type: 'object',
            properties: { id: { type: 'string' } },
            required: ['id']
          },
          defaultPermission: 'ask'
        },
        {
          name: 'run_container',
          label: 'Run container',
          description:
            'Pull the image if needed and create (and start) a new container from it. Supports name, ports, environment variables, volumes, restart policy, network, and command overrides.',
          parameters: {
            type: 'object',
            properties: {
              image: { type: 'string' },
              tag: { type: 'string' },
              name: { type: 'string' },
              command: { type: 'string' },
              detach: { type: 'boolean' },
              restartPolicy: { type: 'string' },
              network: { type: 'string' },
              ports: { type: 'array' },
              env: { type: 'array' },
              volumes: { type: 'array' }
            },
            required: ['image']
          },
          defaultPermission: 'ask'
        },
        {
          name: 'remove_container',
          label: 'Remove container',
          description: 'Force-remove a container by id or name.',
          parameters: {
            type: 'object',
            properties: { id: { type: 'string' } },
            required: ['id']
          },
          defaultPermission: 'ask'
        },
        {
          name: 'pull_image',
          label: 'Pull image',
          description: 'Pull an image from a registry, for example nginx:latest.',
          parameters: {
            type: 'object',
            properties: { image: { type: 'string' } },
            required: ['image']
          },
          defaultPermission: 'ask'
        },
        {
          name: 'remove_image',
          label: 'Remove image',
          description: 'Remove an image by id or reference.',
          parameters: {
            type: 'object',
            properties: { id: { type: 'string' }, force: { type: 'boolean' } },
            required: ['id']
          },
          defaultPermission: 'ask'
        }
      ]
    }
  }
}
