import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import SubagentReport from './SubagentReport.svelte';

describe('SubagentReport', () => {
  it('renders Worker label and omits empty ticketPath', () => {
    const { container } = render(SubagentReport, {
      projectId: 'p',
      route: 'idle',
      report: {
        sessionId: 'ses_w1',
        role: 'worker',
        ticketPath: '',
        status: 'done',
        summary: 'Implemented helper',
        report: 'All tests pass'
      }
    });
    expect(screen.getByText('Worker')).toBeTruthy();
    expect(screen.getByText('Implemented helper')).toBeTruthy();
    expect(container.querySelector('.font-mono')).toBeNull();
  });

  it('renders spec reviewer with ticketPath, error status, and queued route', () => {
    render(SubagentReport, {
      projectId: 'p',
      route: 'pending',
      report: {
        sessionId: 'ses_r1',
        role: 'spec',
        ticketPath: '.scratch/issues/01.md',
        status: 'error',
        summary: 'Spec reviewer failed',
        report: 'Details'
      }
    });
    expect(screen.getByText('Spec reviewer')).toBeTruthy();
    expect(screen.getByText('.scratch/issues/01.md')).toBeTruthy();
    expect(screen.getByText('Sub-agent error')).toBeTruthy();
    expect(screen.getByText('Queued')).toBeTruthy();
  });

  it('renders Scout label and omits empty ticketPath', () => {
    const { container } = render(SubagentReport, {
      projectId: 'p',
      route: 'idle',
      report: {
        sessionId: 'ses_s1',
        role: 'scout',
        ticketPath: '',
        status: 'done',
        summary: 'Mapped routes',
        report: 'Routes live in routes.rs'
      }
    });
    expect(screen.getByText('Scout')).toBeTruthy();
    expect(container.querySelector('.font-mono')).toBeNull();
  });
});
