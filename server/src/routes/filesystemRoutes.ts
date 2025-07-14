import { Router } from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';

const router = Router();
const filesystemController = new FileSystemController();

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/directories', filesystemController.list);

    router.post('/api/directories/:name', filesystemController.mkdir);
    router.delete('/api/directories/:name', filesystemController.rmdir);

    router.post('/api/files/:name', filesystemController.create);
}