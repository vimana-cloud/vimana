"""Test harness and helper functions for E2E tests."""

from datetime import timedelta
from functools import cache
from sys import stderr
from typing import Optional
from unittest import TestCase

import grpc
from kubernetes.client import CoreV1Api
from kubernetes.client.rest import ApiException
from kubernetes.config import load_kube_config


class E2eTestCase(TestCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        load_kube_config()
        cls.coreApi = CoreV1Api()

    @classmethod
    def secureChannel(cls, domain: str, timeout: timedelta = timedelta(seconds=60)):
        channel = grpc.secure_channel(
            domain,
            grpc.ssl_channel_credentials(root_certificates=cls.additionalCas()),
        )
        # Sometimes there is a delay between when the gateway is programmed
        # and when it has finished setting up its listeners, TLS certificates, and routes.
        grpc.channel_ready_future(channel).result(timeout=timeout.total_seconds())
        return channel

    @classmethod
    @cache
    def additionalCas(cls) -> Optional[bytes]:
        # Pull the CA certificate from Pebble if it's running.
        # https://github.com/letsencrypt/pebble/blob/v2.9.0/README.md#ca-root-and-intermediate-certificates
        return cls.getFromService(
            namespace='acme',
            name='pebble',
            path='roots/0',
            port=15000,
            scheme='https',
        )

    @classmethod
    def getFromService(
        cls,
        namespace: str,
        name: str,
        path: str,
        port: Optional[int] = None,
        scheme: Optional[str] = None,
    ) -> Optional[bytes]:
        """
        Send a GET request to the named Kubernetes Service.
        Return the body of the response.

        On error, swallow the exception and return None.
        """
        if port is not None:
            name = f'{name}:{port}'
        if scheme is not None:
            name = f'{scheme}:{name}'

        try:
            return cls.coreApi.connect_get_namespaced_service_proxy_with_path(
                namespace=namespace,
                name=name,
                path=path,
            ).encode()
        except ApiException as error:
            if error.status == 404:
                print(
                    f'Service {name} was not found in namespace {namespace}.',
                    file=stderr,
                )
            else:
                print(
                    f'Error connecting to service proxy for {namespace}/{name}: {error}',
                    file=stderr,
                )
