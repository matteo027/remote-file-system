import express from 'express';
import { setRoutes } from '../src/routes/filesystemRoutes';
import fs from 'fs/promises';
import path from 'path';
import request from 'supertest';

const app = express();
app.use(express.json());
setRoutes(app);

const TEST_DIR = path.join(__dirname, '..', 'file-system/test'); // stesso valore di FS_PATH
const testFile = 'testfile.txt';

beforeAll(async () => {
  await fs.mkdir(TEST_DIR, { recursive: true });
});

afterAll(async () => {
  //await fs.rm(TEST_DIR, { recursive: true, force: true });
});

describe('File System E2E', () => {
  it('should create a file', async () => {
    const res = await request(app)
      .post(`/api/files/${testFile}`)
      .send({ path: 'test' });
    expect(res.status).toBe(200);
  });

  it('should write to the file', async () => {
    const res = await request(app)
      .put(`/api/files/${testFile}`)
      .send({ path: 'test', text: 'ciao mondo' });
    expect(res.status).toBe(200);
  });

  it('should read the file', async () => {
    const res = await request(app)
      .get(`/api/files/${testFile}`)
      .send({ path: 'test' });
    expect(res.status).toBe(200);
    expect(res.body).toEqual({ data: 'ciao mondo' });
  });

  it('should change permissions', async () => {
    const res = await request(app)
      .put(`/api/mod/${testFile}`)
      .send({ path: 'test', new_mod: 0o444 });
    expect(res.status).toBe(200);
    const stats = await fs.stat(path.join(TEST_DIR, testFile));
    const mode = stats.mode & 0o777; // isola i permessi (ultimi 9 bit)

    expect(mode).toBe(0o444);
  });

  it('should deny read and write if permissions are restricted', async () => {

    if (process.platform.startsWith('win')) { // su windows è difficile controllare i permessi
      console.warn('Skipping permission tests on Windows');
      return;
    }


    // settando il file con i permessi --------- (senza alcun permesso)
    await request(app)
      .put(`/api/mod/${testFile}`)
      .send({ path: 'test', new_mod: 0o000 });
    
    // lettura
    const readRes = await request(app)
      .get(`/api/files/${testFile}`)
      .send({ path: 'test' });
    
    const file = await request(app).get(`/api/directories`).send({ path: 'test' });;

    expect(readRes.status).toBe(403);
    expect(readRes.body).toEqual({ error: 'Access denied' });

    // scrittura
    const writeRes = await request(app)
      .put(`/api/files/${testFile}`)
      .send({ path: 'test', text: 'tentativo' });

    expect(writeRes.status).toBe(403);
    expect(writeRes.body).toEqual({ error: 'Access denied' });

    // ripristina i permessi per evitare errori nei test successivi
    await fs.chmod(path.join(TEST_DIR, testFile), 0o644);
  });


  it('should delete the file', async () => {
    const res = await request(app)
      .delete(`/api/files/${testFile}`)
      .send({ path: 'test' });
    expect(res.status).toBe(200);
  });
});
